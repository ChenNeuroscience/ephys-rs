//! Pass triggers, sent by the GUI to a running node's passes (see `crate::core::triggers`): "auto detect", say. A pass of
//! the node takes it (`Pass::triggered`), says how it's getting on (`Pass::trigger_progress`) and when it's done; the
//! GUI shows how far along it is, and can stop it. Each run has its own.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use super::engine::NodeId;
use super::triggers::{AnyTrigger, TriggerKey, TriggersEnum};

/// A run's pass triggers, triggered until they're done.
#[derive(Default)]
pub struct PassTriggers {
    /// How far along each is (0 until a pass takes it).
    triggers: Mutex<HashMap<(NodeId, TriggerKey), PassTrigger>>,
    /// How many there are, so passes needn't take the lock when there are none, as there almost never are. Set
    /// with the lock held (see `Triggers`).
    pending: AtomicUsize,
}

/// The pass triggers, locked, which counts them as it's let go of.
struct Triggers<'a> {
    map: MutexGuard<'a, HashMap<(NodeId, TriggerKey), PassTrigger>>,
    pending: &'a AtomicUsize,
}

impl Drop for Triggers<'_> {
    fn drop(&mut self) {
        self.pending.store(self.map.len(), Ordering::Release);
    }
}

impl std::ops::Deref for Triggers<'_> {
    type Target = HashMap<(NodeId, TriggerKey), PassTrigger>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl std::ops::DerefMut for Triggers<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.map
    }
}

/// A pass trigger triggered: waiting for a pass to take it, or taken, and how far along (0 to 1).
enum PassTrigger {
    Triggered(AnyTrigger),
    Running(f32),
    /// Stopped by the GUI, until a pass sees it has been.
    Cancelled,
}

impl PassTriggers {
    /// Triggers node `node`'s pass trigger `trigger`: its next pass takes it.
    pub fn trigger(&self, node: NodeId, trigger: AnyTrigger) {
        self.triggers().insert((node, trigger.key()), PassTrigger::Triggered(trigger));
    }

    /// How far along node `node`'s trigger of kind `kind` is (0 to 1), if it's been triggered and isn't done (or
    /// stopped).
    pub fn trigger_progress(&self, node: NodeId, kind: impl Into<TriggerKey>) -> Option<f32> {
        if self.pending.load(Ordering::Acquire) == 0 {
            return None;
        }
        self.triggers().get(&(node, kind.into())).and_then(|t| match t {
            PassTrigger::Triggered(_) => Some(0.0),
            PassTrigger::Running(progress) => Some(*progress),
            PassTrigger::Cancelled => None,
        })
    }

    /// Stops node `node`'s trigger of kind `kind`, if it's been triggered: its passes abandon what they were doing.
    pub fn cancel(&self, node: NodeId, kind: impl Into<TriggerKey>) {
        let kind = kind.into();
        let mut triggers = self.triggers();
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
    pub(crate) fn take_cancel(&self, node: NodeId, kind: TriggerKey) -> bool {
        if self.pending.load(Ordering::Acquire) == 0 {
            return false;
        }
        let mut triggers = self.triggers();
        let cancelled = matches!(triggers.get(&(node, kind)), Some(PassTrigger::Cancelled));
        if cancelled {
            triggers.remove(&(node, kind));
        }
        cancelled
    }

    /// For `Pass`: node `node`'s trigger of kind `kind`, of the program's `T`, if it's been triggered, taking it.
    pub(crate) fn take_trigger<T: TriggersEnum>(&self, node: NodeId, kind: T::Kind) -> Option<T> {
        if self.pending.load(Ordering::Acquire) == 0 {
            return None;
        }
        let mut triggers = self.triggers();
        let state = triggers.get_mut(&(node, kind.into()))?;
        if !matches!(state, PassTrigger::Triggered(_)) {
            return None;
        }
        let PassTrigger::Triggered(trigger) = std::mem::replace(state, PassTrigger::Running(0.0)) else { return None };
        trigger.take::<T>()
    }

    /// For `Pass`: how far along a trigger taken is, or (`None`) that it's done.
    pub(crate) fn set_trigger_progress(&self, node: NodeId, kind: TriggerKey, progress: Option<f32>) {
        let mut triggers = self.triggers();
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

    /// The pass triggers, locked; counted in `pending` when let go of.
    fn triggers(&self) -> Triggers<'_> {
        Triggers { map: self.triggers.lock().unwrap(), pending: &self.pending }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_trigger_is_taken_with_what_it_carries_once() {
        use crate::core::triggers::testing::{TestKind, TestTrigger};
        let triggers = PassTriggers::default();
        let node = NodeId(1);
        triggers.trigger(node, AnyTrigger::new(TestTrigger::Level(0.5)));
        assert_eq!(triggers.trigger_progress(node, TestKind::Level), Some(0.0), "waiting");
        assert_eq!(triggers.take_trigger::<TestTrigger>(NodeId(2), TestKind::Level), None, "another node's");
        assert_eq!(triggers.take_trigger(node, TestKind::Level), Some(TestTrigger::Level(0.5)));
        assert_eq!(triggers.take_trigger::<TestTrigger>(node, TestKind::Level), None, "taken");
        assert_eq!(triggers.trigger_progress(node, TestKind::Level), Some(0.0), "running, till it's done");
        triggers.set_trigger_progress(node, TestKind::Level.into(), None);
        assert_eq!(triggers.trigger_progress(node, TestKind::Level), None);
    }
}
