//! How far along nodes are with what they've been asked to do over a while, with the data going by (an auto detect,
//! say). The asking is a transient setting, which says when it was asked (see `EdgeType::clear_transient`); the node's
//! passes report how they're getting on with it (`PassContext::report_progress`), and windows show it
//! (`UiContext::progress`). It's the run's.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use super::NodeId;

/// How far along each node is with what it was asked to do, and when it was asked.
#[derive(Default)]
pub struct Progress {
    by_node: Mutex<HashMap<NodeId, (Instant, f32)>>,
}

impl Progress {
    /// For `PassContext`: node `node` is `fraction` of the way through what it was asked to do at `asked`.
    pub(crate) fn report(&self, node: NodeId, asked: Instant, fraction: f32) {
        self.by_node.lock().unwrap().insert(node, (asked, fraction.clamp(0.0, 1.0)));
    }

    /// How far node `node` is through what it was asked to do at `asked`, from 0 (as nothing's been said about it yet)
    /// to 1.
    pub fn of(&self, node: NodeId, asked: Instant) -> f32 {
        match self.by_node.lock().unwrap().get(&node) {
            Some(&(was, fraction)) if was == asked => fraction,
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_of_what_was_asked_when() {
        let progress = Progress::default();
        let (node, first) = (NodeId(1), Instant::now());
        assert_eq!(progress.of(node, first), 0.0, "nothing said yet");
        progress.report(node, first, 0.25);
        assert_eq!(progress.of(node, first), 0.25);
        assert_eq!(progress.of(NodeId(2), first), 0.0, "another node's");
        let again = first + std::time::Duration::from_secs(1);
        assert_eq!(progress.of(node, again), 0.0, "asked again: none of that yet");
        progress.report(node, again, 2.0);
        assert_eq!(progress.of(node, again), 1.0, "at most all the way");
    }
}
