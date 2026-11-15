//! Snapshot triggers (see `crate::triggers`): "auto detect", say, which a node does with the data going by, while
//! running. Sending one goes to the app, which keeps the triggers sent (`SentTriggers`, one a node at most), and bakes
//! a new snapshot carrying them. A node prepares for its trigger like anything else in a snapshot
//! (`PrepareContext::trigger`: the sampling buffers an auto detect needs, say), runs while it has one (the tree shake's
//! `triggered`), and its passes say how it's getting on, and when it's done (`PassContext::trigger_progress`,
//! `trigger_done`), in the run's `TriggerProgress`. Once it's done, the app takes the trigger away, which is another
//! snapshot; until that takes over, the node's state knows it's done. Stopping one is the app taking it away early.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use super::engine::NodeId;
use super::triggers::{TriggerKey, TriggersEnum};

/// A snapshot trigger sent to a node, of the program's `T`: each sent gets an id of its own, so a node can tell one
/// sent again from the one before.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent<T> {
    pub id: u64,
    pub key: TriggerKey,
    pub trigger: T,
}

/// The snapshot triggers a snapshot carries, by node.
pub type SnapshotTriggers<T> = BTreeMap<NodeId, Sent<T>>;

/// The app's snapshot triggers: those sent and not yet done or stopped, one a node at most.
pub struct SentTriggers<T> {
    next_id: u64,
    sent: Arc<SnapshotTriggers<T>>,
}

impl<T> Default for SentTriggers<T> {
    fn default() -> Self {
        SentTriggers { next_id: 1, sent: Arc::default() }
    }
}

impl<T: TriggersEnum> SentTriggers<T> {
    /// Sends node `node` `trigger`, in place of any it has.
    pub fn send(&mut self, node: NodeId, trigger: T) {
        let sent = Sent { id: self.next_id, key: trigger.kind().into(), trigger };
        self.next_id += 1;
        Arc::make_mut(&mut self.sent).insert(node, sent);
    }

    /// Takes node `node`'s trigger away, if it has one: it's done, or stopped. Returns whether it had one.
    pub fn remove(&mut self, node: NodeId) -> bool {
        self.sent.contains_key(&node) && Arc::make_mut(&mut self.sent).remove(&node).is_some()
    }

    /// Takes them all away (as the run stops). Returns whether there were any.
    pub fn clear(&mut self) -> bool {
        let any = !self.sent.is_empty();
        self.sent = Arc::default();
        any
    }

    /// Those sent, for a snapshot to carry.
    pub fn current(&self) -> &Arc<SnapshotTriggers<T>> {
        &self.sent
    }
}

/// How a snapshot trigger is getting on, as its node's passes say.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Progress {
    /// From 0 to 1.
    Running(f32),
    Done,
}

/// How the snapshot triggers are getting on, by node: the run's, which its passes report to, and the app reads.
#[derive(Default)]
pub struct TriggerProgress {
    /// By node: the trigger's id, and how it's getting on.
    progress: Mutex<HashMap<NodeId, (u64, Progress)>>,
}

impl TriggerProgress {
    /// For `PassContext`: node `node`'s trigger `id` is `progress` of the way along (0 to 1).
    pub(crate) fn set(&self, node: NodeId, id: u64, progress: f32) {
        let mut all = self.progress.lock().unwrap();
        // Not over its being done, by a pass after: passes needn't finish in order.
        if all.get(&node) != Some(&(id, Progress::Done)) {
            all.insert(node, (id, Progress::Running(progress.clamp(0.0, 1.0))));
        }
    }

    /// For `PassContext`: node `node`'s trigger `id` is done.
    pub(crate) fn done(&self, node: NodeId, id: u64) {
        self.progress.lock().unwrap().insert(node, (id, Progress::Done));
    }

    /// How far along node `node`'s trigger `id` is: 0 until a pass says, and 1 once it's done.
    pub fn of(&self, node: NodeId, id: u64) -> f32 {
        match self.progress.lock().unwrap().get(&node) {
            Some((was, Progress::Running(progress))) if *was == id => *progress,
            Some((was, Progress::Done)) if *was == id => 1.0,
            _ => 0.0,
        }
    }

    /// The nodes whose triggers among `sent` are done, for the app to take away.
    pub fn finished<T>(&self, sent: &SnapshotTriggers<T>) -> Vec<NodeId> {
        let all = self.progress.lock().unwrap();
        let done = |node: &NodeId, id: u64| all.get(node) == Some(&(id, Progress::Done));
        sent.iter().filter(|(node, sent)| done(node, sent.id)).map(|(node, _)| *node).collect()
    }
}

/// The snapshot triggers sent, and how they're getting on, while running: what a node's window sees of them.
pub struct LiveTriggers<'a, T> {
    pub sent: &'a SnapshotTriggers<T>,
    pub progress: &'a TriggerProgress,
}

impl<T> Clone for LiveTriggers<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for LiveTriggers<'_, T> {}

impl<T> LiveTriggers<'_, T> {
    /// How far along node `node`'s trigger of kind `kind` is (0 to 1), if it's been sent and isn't done.
    pub fn progress(&self, node: NodeId, kind: TriggerKey) -> Option<f32> {
        let sent = self.sent.get(&node).filter(|sent| sent.key == kind)?;
        Some(self.progress.of(node, sent.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triggers::testing::{TestKind, TestTrigger};

    #[test]
    fn a_trigger_sent_is_carried_till_it_is_done_and_taken_away() {
        let (mut sent, progress) = (SentTriggers::<TestTrigger>::default(), TriggerProgress::default());
        let node = NodeId(1);
        sent.send(node, TestTrigger::Level(0.5));
        let first = sent.current().clone();
        let id = first[&node].id;
        let live = LiveTriggers { sent: &first, progress: &progress };
        assert_eq!(live.progress(node, TestKind::Level.into()), Some(0.0), "sent, before a pass says");
        assert_eq!(live.progress(NodeId(2), TestKind::Level.into()), None, "another node's");
        progress.set(node, id, 0.25);
        assert_eq!(live.progress(node, TestKind::Level.into()), Some(0.25));
        assert!(progress.finished(&first).is_empty());
        progress.done(node, id);
        progress.set(node, id, 0.5);
        assert_eq!(live.progress(node, TestKind::Level.into()), Some(1.0), "done, whatever a later pass says");
        assert_eq!(progress.finished(&first), vec![node]);
        // Sent again: a new one, which the old one's being done says nothing about.
        sent.send(node, TestTrigger::Level(0.5));
        assert_ne!(sent.current()[&node].id, id);
        assert!(progress.finished(sent.current()).is_empty());
        assert!(sent.remove(node) && !sent.remove(node));
        assert_eq!(first.len(), 1, "a snapshot's are its own");
    }
}
