//! The quick actions offered in the basic view: adding spike, EEG or position tracking, each with the nodes it
//! needs, recorded by Write LTX. Each reuses what the graph already has (an acquisition board, a spike detector, a
//! Write LTX...), adding only what's missing, and connecting only ports that are free; it's not offered once the
//! graph does all it would. Each opens its viewer.

use crate::Nodes;
use crate::core::engine::{Graph, NodeId, NodeSet, Port, Role};
use crate::core::quick_action::{QuickAction, QuickChange, QuickState};

pub const QUICK_ACTIONS: &[QuickAction<Nodes>] = &[
    QuickAction {
        name: "Add spike tracking",
        description: "Spikes from the acquisition board's tetrodes, re-referenced and filtered, viewed and recorded",
        run: spikes,
    },
    QuickAction {
        name: "Add EEG tracking",
        description: "The acquisition board's channels as LFP (1–300 Hz), viewed with their spectra and recorded",
        run: eeg,
    },
    QuickAction {
        name: "Add position tracking",
        description: "The camera's LEDs, tracked, viewed and recorded",
        run: positions,
    },
];

/// Room from one node to the next along the data's flow, and between nodes one above the other.
const STEP: f32 = 280.0;
const NODE_HEIGHT: f32 = 150.0;

/// The graph being changed, and whether it has been.
struct Change {
    graph: Graph<Nodes>,
    changed: bool,
}

impl Change {
    fn new(graph: &Graph<Nodes>) -> Self {
        Change { graph: graph.clone(), changed: false }
    }

    /// The first node that's `matches`.
    fn find(&self, matches: impl Fn(&Nodes) -> bool) -> Option<NodeId> {
        self.graph.iter().find(|(_, e)| matches(&e.node)).map(|(id, _)| id)
    }

    /// The first node of type `name`, or a new one, after `after` (if given).
    fn upsert(&mut self, name: &str, after: Option<NodeId>) -> NodeId {
        let index = Nodes::TYPES.iter().position(|t| t.name == name).expect("node type");
        let found = self.graph.iter().find(|(_, e)| e.node.info().name == name).map(|(id, _)| id);
        found.unwrap_or_else(|| self.add(Nodes::create(index), after))
    }

    /// Adds `node` after `after` along the data's flow, below whatever's there; or, with nothing before it, below
    /// the whole graph, at its left.
    fn add(&mut self, node: Nodes, after: Option<NodeId>) -> NodeId {
        let positions: Vec<[f32; 2]> = self.graph.iter().map(|(_, e)| e.pos).collect();
        let mut pos = match after.and_then(|id| self.graph.get(id)) {
            Some(entry) => [entry.pos[0] + STEP, entry.pos[1]],
            None => {
                let left = positions.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
                let bottom = positions.iter().map(|p| p[1] + NODE_HEIGHT + 60.0).fold(f32::NEG_INFINITY, f32::max);
                if positions.is_empty() { [0.0, 0.0] } else { [left, bottom] }
            }
        };
        let taken = |pos: [f32; 2]| {
            positions.iter().any(|p| (p[0] - pos[0]).abs() < STEP * 0.9 && (p[1] - pos[1]).abs() < NODE_HEIGHT)
        };
        while taken(pos) {
            pos[1] += 40.0;
        }
        self.changed = true;
        self.graph.add(node, pos)
    }

    /// Whether a port has an edge.
    fn linked(&self, port: Port) -> bool {
        self.graph.edges().iter().any(|e| e.source() == port || e.target() == port)
    }

    /// Connects an output to an input (or a control to a setting), if the input (or control) is free.
    fn link(&mut self, from: (NodeId, Role, usize), to: (NodeId, Role, usize)) {
        let (from, to) = (port(from), port(to));
        let free = match to.role {
            Role::Input => !self.linked(to),
            _ => !self.linked(from),
        };
        if free && self.graph.connect(from, to).is_ok() {
            self.changed = true;
        }
    }

    /// The change, opening `window` (unless it's open already); nothing if nothing's changed (the graph already does
    /// it all).
    fn finish(self, state: &QuickState<Nodes>, window: NodeId) -> Option<QuickChange<Nodes>> {
        let open = if state.open_windows.contains(&window) { vec![] } else { vec![window] };
        self.changed.then_some(QuickChange { graph: self.graph, open })
    }
}

fn port((node, role, index): (NodeId, Role, usize)) -> Port {
    Port { node, role, index }
}

use Role::{Control, Input, Output, Setting};

/// Write LTX, recording the board's digital inputs too.
fn write_ltx(change: &mut Change, board: Option<NodeId>, after: NodeId) -> NodeId {
    let ltx = change.upsert("Write LTX", Some(after));
    if let Some(board) = board {
        change.link((board, Output, 1), (ltx, Input, 3));
    }
    ltx
}

/// Board → Reference → Band-pass (spikes' band) → Spike Detector → Spike Viewer (setting the detector's thresholds
/// and gains), with Write LTX recording the spikes at the detector's gains.
fn spikes(state: &QuickState<Nodes>) -> Option<QuickChange<Nodes>> {
    let mut change = Change::new(state.graph);
    let board = change.upsert("Acquisition Board", None);
    let reference = change.upsert("Reference", Some(board));
    change.link((board, Output, 0), (reference, Input, 0));
    let band = match change.find(|n| matches!(n, Nodes::BandPassFilter(f) if f.low_hz >= 100.0)) {
        Some(band) => band,
        None => change.add(Nodes::BandPassFilter(Default::default()), Some(reference)),
    };
    change.link((reference, Output, 0), (band, Input, 0));
    let detector = change.upsert("Spike Detector", Some(band));
    change.link((band, Output, 0), (detector, Input, 0));
    let viewer = change.upsert("Spike Viewer", Some(detector));
    change.link((detector, Output, 0), (viewer, Input, 0));
    change.link((viewer, Control, 0), (detector, Setting, 0));
    change.link((viewer, Control, 1), (detector, Setting, 1));
    let ltx = write_ltx(&mut change, Some(board), detector);
    change.link((detector, Output, 0), (ltx, Input, 0));
    change.link((ltx, Control, 1), (detector, Setting, 1));
    change.finish(state, viewer)
}

/// Board → Channel Selector → Band-pass (LFP's band, 1–300 Hz) → LFP Viewer (choosing the selector's channels), with
/// Write LTX recording the LFP as EEG.
fn eeg(state: &QuickState<Nodes>) -> Option<QuickChange<Nodes>> {
    let mut change = Change::new(state.graph);
    let board = change.upsert("Acquisition Board", None);
    let selector = change.upsert("Channel Selector", Some(board));
    change.link((board, Output, 0), (selector, Input, 0));
    let lfp = |n: &Nodes| matches!(n, Nodes::BandPassFilter(f) if f.high_hz <= 500.0);
    let band = match change.find(lfp) {
        Some(band) => band,
        None => {
            let filter = crate::standard_components::ephys::band_pass::BandPassFilter {
                low_hz: 1.0,
                high_hz: 300.0,
                ..Default::default()
            };
            change.add(Nodes::BandPassFilter(filter), Some(selector))
        }
    };
    change.link((selector, Output, 0), (band, Input, 0));
    let viewer = change.upsert("LFP Viewer", Some(band));
    change.link((band, Output, 0), (viewer, Input, 0));
    change.link((viewer, Control, 1), (selector, Setting, 0));
    let ltx = write_ltx(&mut change, Some(board), band);
    change.link((band, Output, 0), (ltx, Input, 1));
    change.finish(state, viewer)
}

/// Camera → Position Detector → Tracker View (setting the camera's and detector's settings), with Write LTX
/// recording the positions, and the detector's settings with them.
fn positions(state: &QuickState<Nodes>) -> Option<QuickChange<Nodes>> {
    let mut change = Change::new(state.graph);
    let camera = change.upsert("Camera", None);
    let detector = change.upsert("Position Detector", Some(camera));
    change.link((camera, Output, 0), (detector, Input, 0));
    let view = change.upsert("Tracker View", Some(detector));
    change.link((camera, Output, 0), (view, Input, 0));
    change.link((detector, Output, 0), (view, Input, 1));
    change.link((camera, Output, 1), (view, Input, 2));
    change.link((view, Control, 0), (camera, Setting, 0));
    change.link((view, Control, 1), (detector, Setting, 0));
    let board = change.find(|n| matches!(n, Nodes::AcquisitionBoard(_)));
    let ltx = write_ltx(&mut change, board, detector);
    change.link((detector, Output, 0), (ltx, Input, 2));
    change.link((ltx, Control, 0), (detector, Setting, 0));
    change.finish(state, view)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn run(action: usize, graph: &Graph<Nodes>) -> Option<QuickChange<Nodes>> {
        (QUICK_ACTIONS[action].run)(&QuickState { graph, open_windows: &HashSet::new() })
    }

    fn count(graph: &Graph<Nodes>, name: &str) -> usize {
        graph.iter().filter(|(_, e)| e.node.info().name == name).count()
    }

    #[test]
    fn each_adds_what_it_needs_once_and_shares_the_rest() {
        let mut graph = Graph::default();
        for action in 0..3 {
            let change = run(action, &graph).expect("offered on a graph without it");
            assert_eq!(change.open.len(), 1);
            assert!(change.graph.get(change.open[0]).unwrap().node.has_view(), "opens a viewer");
            graph = change.graph;
            assert!(run(action, &graph).is_none(), "not offered again once done");
        }
        for (name, n) in [("Acquisition Board", 1), ("Write LTX", 1), ("Band-pass Filter", 2), ("Camera", 1)] {
            assert_eq!(count(&graph, name), n, "{name}");
        }
        let resolved = graph.resolve();
        assert!(graph.iter().all(|(id, e)| e.node.warnings(&resolved[&id].input_streams).is_empty()), "all connected");
        // Write LTX records all four, and the board's digital inputs.
        let ltx = graph.iter().find(|(_, e)| e.node.info().name == "Write LTX").unwrap().0;
        let inputs = (0..4).filter(|&i| graph.edges().iter().any(|e| e.target() == port((ltx, Input, i)))).count();
        assert_eq!(inputs, 4);
    }

    #[test]
    fn the_example_already_does_them_all() {
        let graph = crate::example_graph();
        for (index, action) in QUICK_ACTIONS.iter().enumerate() {
            assert!(run(index, &graph).is_none(), "{}", action.name);
        }
    }
}
