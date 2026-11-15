//! What node code can see of a snapshot (`SnapshotLite`): a subset of it, its version, and the run's start and recording. A pass gives
//! a node its own (`PassContext::snapshot`), shared, so a node's thread can keep it: frozen, as it was baked. (The
//! rest, its graph and plan, are of the program's node types, which nodes don't know.)

use std::time::{Duration, Instant};

use super::{NodesEnum, Recording, Snapshot};

/// What node code can see of a snapshot: a subset of it.
pub trait SnapshotLite: Send + Sync {
    fn version(&self) -> u64;

    /// When the run started (Play pressed), by the monotonic clock, while running.
    fn started(&self) -> Option<Instant>;

    /// The recording, while recording.
    fn recording(&self) -> Option<&Recording>;

    /// When the recording took effect (see `Recording::effective`), by the same clock as `started`, once it has.
    fn recording_started(&self) -> Option<Instant> {
        Some(self.recording()?.effective()?.at)
    }

    /// How long it's been acquiring, now, while running.
    fn acquiring_for(&self) -> Option<Duration> {
        Some(self.started()?.elapsed())
    }

    /// How long it's been recording, now, while recording.
    fn recording_for(&self) -> Option<Duration> {
        Some(self.recording_started()?.elapsed())
    }
}

impl<N: NodesEnum> SnapshotLite for Snapshot<N> {
    fn version(&self) -> u64 {
        self.version
    }

    fn started(&self) -> Option<Instant> {
        self.started
    }

    fn recording(&self) -> Option<&Recording> {
        self.recording.as_deref()
    }
}

/// For trying a node out (see `PassContext::new`): started now, and recording or not.
#[cfg(test)]
pub(super) struct Trying {
    pub started: Instant,
    pub recording: Option<std::sync::Arc<Recording>>,
}

#[cfg(test)]
impl SnapshotLite for Trying {
    fn version(&self) -> u64 {
        0
    }

    fn started(&self) -> Option<Instant> {
        Some(self.started)
    }

    fn recording(&self) -> Option<&Recording> {
        self.recording.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Graph, Recording};
    use std::sync::Arc;

    #[test]
    fn it_says_how_long_as_of_now_from_the_times_baked() {
        let (graph, started) = (Arc::new(Graph::default()), Instant::now() - Duration::from_secs(5));
        let running = Snapshot::<crate::Nodes>::bake(1, graph.clone(), Some(started), None);
        assert!(running.acquiring_for().unwrap() >= Duration::from_secs(5));
        assert_eq!((running.version(), running.recording_for()), (1, None));
        let recorded = Arc::new(Recording::default());
        let recording = Snapshot::<crate::Nodes>::bake(2, graph.clone(), Some(started), Some(recorded.clone()));
        assert_eq!(recording.recording_started(), None, "not until it takes effect");
        let at = Instant::now() - Duration::from_millis(300);
        recorded.take_effect(at);
        recorded.take_effect(Instant::now());
        assert_eq!(recording.recording_started(), Some(at), "when it first took effect");
        assert!(recording.recording_for().unwrap() < Duration::from_secs(1));
        assert_eq!(Snapshot::<crate::Nodes>::bake(3, graph, None, None).acquiring_for(), None, "not running");
    }
}
