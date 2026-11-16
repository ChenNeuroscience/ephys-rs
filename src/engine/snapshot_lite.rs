//! A node can see some of the snapshot a pass runs against (`SnapshotLite`): its version, and the run's start and
//! recording. A pass gives the node the snapshot shared (`PassContext::snapshot`), so a node's thread can keep it,
//! frozen as it was baked. (The rest, its graph and plan, are of the program's node types, which nodes don't know.)

use std::time::{Duration, Instant};

use super::{NodesEnum, Recording, Snapshot};

/// Node code can see this much of a snapshot.
pub trait SnapshotLite: Send + Sync {
    /// The app numbers the snapshots it bakes, in order.
    fn version(&self) -> u64;

    /// While running, the run started (Play pressed) at this time, by the monotonic clock.
    fn started(&self) -> Option<Instant>;

    /// While recording, the recording.
    fn recording(&self) -> Option<&Recording>;

    /// Once the recording's taken effect (see `Recording::effective`), it did so at this time, by the same clock as
    /// `started`.
    fn recording_started(&self) -> Option<Instant> {
        Some(self.recording()?.effective()?.at)
    }

    /// While running, the graph's been acquiring for this long, as of now.
    fn acquiring_for(&self) -> Option<Duration> {
        Some(self.started()?.elapsed())
    }

    /// While recording, the graph's been recording for this long, as of now.
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

/// Tests try nodes out against a snapshot like this (see `PassContext::new`): started now, and recording or not.
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
