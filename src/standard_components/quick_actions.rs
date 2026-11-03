//! The quick actions offered in the basic view:
//!
//! - The standard LTX graph (v0.5): an Open Ephys acquisition board (generation 1 or 2) giving spikes, EEG and digital
//!   inputs, and a FLIR camera giving positions, all viewed, and recorded by Write LTX. It reuses what the graph
//!   already has, adding only what's missing (and connecting only ports that are free), so it's offered until the
//!   graph has it all. It arranges the panes: the tracker view in a column at the left (with home as a tab behind
//!   it); the spike viewer filling the rest above (with any panes open before as tabs behind it), and the LFP viewer
//!   below that (with the TTL viewer as a tab behind it).
//! - Disabling position tracking (the camera, position detector and tracker view), while it's all enabled; and
//!   enabling it, while any of it isn't.
//! - Using synthetic data (the fake board and camera in `synthetic_streams`), while any board or camera is real; and
//!   real devices, while any is synthetic.

use crate::Nodes;
use crate::core::engine::{Graph, NodeId, NodeSet, Port, Role};
use crate::core::quick_action::{Arrangement, QuickAction, QuickChange, QuickState, Tab};
use crate::standard_components::ephys::band_pass::BandPassFilter;
use crate::standard_components::ephys::board::{self, node::AcquisitionBoard, opal_kelly::OpalKellyKind};
use crate::standard_components::tracking::camera::{self, node::Camera};

pub const QUICK_ACTIONS: &[QuickAction<Nodes>] = &[
    QuickAction {
        name: "Standard LTX graph v0.5",
        description: "An Open Ephys acquisition board (gen 1/2) and a FLIR camera: spikes, EEG, digital inputs and \
                      positions, viewed and recorded by Write LTX",
        run: standard,
    },
    QuickAction {
        name: "Disable position tracking",
        description: "Run without the camera, position detector and tracker view, keeping them",
        run: disable_positions,
    },
    QuickAction {
        name: "Enable position tracking",
        description: "Run with the camera, position detector and tracker view again",
        run: enable_positions,
    },
    QuickAction {
        name: "Use synthetic data",
        description: "The fake acquisition board and camera (run synthetic_streams' scripts), instead of real devices",
        run: synthetic,
    },
    QuickAction {
        name: "Use real devices",
        description: "An Open Ephys acquisition board (gen 1/2) and a FLIR camera, instead of the synthetic data",
        run: real,
    },
];

/// Room from one node to the next along the data's flow, and between nodes one above the other.
const STEP: f32 = 280.0;
const NODE_HEIGHT: f32 = 150.0;

/// The graph being changed, and what's been done to it.
struct Change {
    graph: Graph<Nodes>,
    changed: bool,
    added: Vec<String>,
    connected: usize,
}

impl Change {
    fn new(graph: &Graph<Nodes>) -> Self {
        Change { graph: graph.clone(), changed: false, added: Vec::new(), connected: 0 }
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
        let id = self.graph.add(node, pos);
        self.added.extend(self.graph.get(id).map(|e| e.label.clone()));
        id
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
            self.connected += 1;
        }
    }

    /// The change, opening `windows` (those not open already); nothing if nothing's changed (the graph already
    /// does it all).
    fn finish(self, state: &QuickState<Nodes>, windows: &[NodeId]) -> Option<QuickChange<Nodes>> {
        let open = windows.iter().copied().filter(|w| !state.open_windows.contains(w)).collect();
        let mut done = Vec::new();
        if !self.added.is_empty() {
            done.push(format!("added {}", self.added.join(", ")));
        }
        if self.connected > 0 {
            done.push(format!("connected {}", self.connected));
        }
        let summary = done.join("; ");
        self.changed.then_some(QuickChange {
            graph: self.graph,
            open,
            arrange: false,
            panes: None,
            close: Vec::new(),
            summary,
        })
    }
}

fn port((node, role, index): (NodeId, Role, usize)) -> Port {
    Port { node, role, index }
}

use Role::{Control, Input, Output, Setting};

/// A real board: the Open Ephys acquisition board, generation 1 or 2.
fn real_board() -> board::Source {
    board::Source::OpalKelly { board: OpalKellyKind::OpenEphys, bitfile: String::new(), library: String::new() }
}

/// A real camera: the first FLIR camera found.
fn real_camera() -> camera::Source {
    camera::Source::Spinnaker { serial: String::new(), library: String::new() }
}

/// Board → Channel Selector → Reference → Band-pass (spikes' band) → Spike Detector → Spike Viewer (setting the
/// detector's thresholds and gains); the selector → Band-pass (LFP's band) → LFP Viewer (choosing the selector's
/// channels); the board's digital inputs → TTL Viewer; Camera → Position Detector → Tracker View (setting the
/// camera's and detector's settings). Write LTX records the spikes (at the detector's gains), the referenced data as
/// EEG, the positions (with the detector's settings) and the digital inputs. New boards and cameras are real.
fn standard(state: &QuickState<Nodes>) -> Option<QuickChange<Nodes>> {
    let mut change = Change::new(state.graph);
    let empty = state.graph.iter().next().is_none();
    let board = match change.find(|n| matches!(n, Nodes::AcquisitionBoard(_))) {
        Some(board) => board,
        None => {
            change.add(Nodes::AcquisitionBoard(AcquisitionBoard { source: real_board(), ..Default::default() }), None)
        }
    };
    let selector = change.upsert("Channel Selector", Some(board));
    change.link((board, Output, 0), (selector, Input, 0));
    let reference = change.upsert("Reference", Some(selector));
    change.link((selector, Output, 0), (reference, Input, 0));

    // Spikes.
    let band = match change.find(|n| matches!(n, Nodes::BandPassFilter(f) if f.low_hz >= 100.0)) {
        Some(band) => band,
        None => change.add(Nodes::BandPassFilter(Default::default()), Some(reference)),
    };
    change.link((reference, Output, 0), (band, Input, 0));
    let detector = change.upsert("Spike Detector", Some(band));
    change.link((band, Output, 0), (detector, Input, 0));
    let spike_viewer = change.upsert("Spike Viewer", Some(detector));
    change.link((detector, Output, 0), (spike_viewer, Input, 0));
    change.link((spike_viewer, Control, 0), (detector, Setting, 0));
    change.link((spike_viewer, Control, 1), (detector, Setting, 1));

    // EEG.
    let lfp = match change.find(|n| matches!(n, Nodes::BandPassFilter(f) if f.high_hz <= 500.0)) {
        Some(lfp) => lfp,
        None => {
            let filter = BandPassFilter { low_hz: 1.0, high_hz: 300.0, ..Default::default() };
            change.add(Nodes::BandPassFilter(filter), Some(selector))
        }
    };
    change.link((selector, Output, 0), (lfp, Input, 0));
    let lfp_viewer = change.upsert("LFP Viewer", Some(lfp));
    change.link((lfp, Output, 0), (lfp_viewer, Input, 0));
    change.link((lfp_viewer, Control, 1), (selector, Setting, 0));

    // Digital inputs.
    let ttl_viewer = change.upsert("TTL Viewer", Some(board));
    change.link((board, Output, 1), (ttl_viewer, Input, 0));

    // Positions.
    let camera = match change.find(|n| matches!(n, Nodes::Camera(_))) {
        Some(camera) => camera,
        None => change.add(Nodes::Camera(Camera { source: real_camera(), ..Default::default() }), None),
    };
    let positions = change.upsert("Position Detector", Some(camera));
    change.link((camera, Output, 0), (positions, Input, 0));
    let tracker = change.upsert("Tracker View", Some(positions));
    change.link((camera, Output, 0), (tracker, Input, 0));
    change.link((positions, Output, 0), (tracker, Input, 1));
    change.link((camera, Output, 1), (tracker, Input, 2));
    change.link((tracker, Control, 0), (camera, Setting, 0));
    change.link((tracker, Control, 1), (positions, Setting, 0));

    // Recording it all.
    let ltx = change.upsert("Write LTX", Some(detector));
    change.link((detector, Output, 0), (ltx, Input, 0));
    change.link((reference, Output, 0), (ltx, Input, 1));
    change.link((positions, Output, 0), (ltx, Input, 2));
    change.link((board, Output, 1), (ltx, Input, 3));
    change.link((ltx, Control, 0), (positions, Setting, 0));
    change.link((ltx, Control, 1), (detector, Setting, 1));

    let mut result = change.finish(state, &[spike_viewer, lfp_viewer, ttl_viewer, tracker])?;
    // A new graph, laid out tidily.
    result.arrange = empty;
    let tabs = |tabs: Vec<Tab>| Arrangement::Tabs { tabs, active: 0 };
    result.panes = Some(Arrangement::Row(vec![
        (0.3, Arrangement::Tabs { tabs: vec![Tab::Home, Tab::Window(tracker)], active: 1 }),
        (
            0.7,
            Arrangement::Column(vec![
                (0.7, tabs(vec![Tab::Window(spike_viewer), Tab::Others])),
                (0.3, tabs(vec![Tab::Window(lfp_viewer), Tab::Window(ttl_viewer)])),
            ]),
        ),
    ]));
    Some(result)
}

/// The position tracking nodes: cameras, position detectors and tracker views.
fn position_nodes(graph: &Graph<Nodes>) -> Vec<NodeId> {
    graph
        .iter()
        .filter(|(_, e)| matches!(e.node, Nodes::Camera(_) | Nodes::PositionDetector(_) | Nodes::TrackerView(_)))
        .map(|(id, _)| id)
        .collect()
}

/// Sets whether the position tracking nodes are disabled, if it's offered: disabling while they're all enabled,
/// enabling while any isn't.
fn set_positions_disabled(state: &QuickState<Nodes>, disabled: bool) -> Option<QuickChange<Nodes>> {
    let ids = position_nodes(state.graph);
    let any_disabled = ids.iter().any(|id| state.graph.get(*id).is_some_and(|e| e.disabled));
    if ids.is_empty() || any_disabled == disabled {
        return None;
    }
    let mut graph = state.graph.clone();
    for id in &ids {
        if let Some(entry) = graph.get_mut(*id) {
            entry.disabled = disabled;
        }
    }
    // Enabled, the tracker view's worth seeing; disabled, it isn't.
    let trackers: Vec<NodeId> = ids
        .iter()
        .copied()
        .filter(|id| matches!(graph.get(*id).map(|e| &e.node), Some(Nodes::TrackerView(_))))
        .collect();
    let (open, close) = if disabled { (Vec::new(), trackers) } else { (trackers, Vec::new()) };
    let names: Vec<String> = ids.iter().filter_map(|id| graph.get(*id).map(|e| e.label.clone())).collect();
    let summary = format!("{} {}", if disabled { "disabled" } else { "enabled" }, names.join(", "));
    Some(QuickChange { graph, open, close, arrange: false, panes: None, summary })
}

fn disable_positions(state: &QuickState<Nodes>) -> Option<QuickChange<Nodes>> {
    set_positions_disabled(state, true)
}

fn enable_positions(state: &QuickState<Nodes>) -> Option<QuickChange<Nodes>> {
    set_positions_disabled(state, false)
}

/// Sets every board and camera to the synthetic data or to real devices, if any isn't already.
fn set_synthetic(state: &QuickState<Nodes>, synthetic: bool) -> Option<QuickChange<Nodes>> {
    let mut graph = state.graph.clone();
    let mut changed = Vec::new();
    for id in graph.ids().collect::<Vec<_>>() {
        let Some(entry) = graph.get_mut(id) else { continue };
        match &mut entry.node {
            Nodes::AcquisitionBoard(b) if matches!(b.source, board::Source::Fake { .. }) != synthetic => {
                b.source = if synthetic { board::Source::default() } else { real_board() };
                changed.push(entry.label.clone());
            }
            Nodes::Camera(c) if matches!(c.source, camera::Source::Fake { .. }) != synthetic => {
                c.source = if synthetic { camera::Source::default() } else { real_camera() };
                changed.push(entry.label.clone());
            }
            _ => {}
        }
    }
    let summary = format!("{} now {}", changed.join(", "), if synthetic { "synthetic" } else { "real devices" });
    (!changed.is_empty()).then_some(QuickChange {
        graph,
        open: Vec::new(),
        arrange: false,
        panes: None,
        close: Vec::new(),
        summary,
    })
}

fn synthetic(state: &QuickState<Nodes>) -> Option<QuickChange<Nodes>> {
    set_synthetic(state, true)
}

fn real(state: &QuickState<Nodes>) -> Option<QuickChange<Nodes>> {
    set_synthetic(state, false)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn run(name: &str, graph: &Graph<Nodes>) -> Option<QuickChange<Nodes>> {
        let action = QUICK_ACTIONS.iter().find(|a| a.name == name).expect("action");
        (action.run)(&QuickState { graph, open_windows: &HashSet::new() })
    }

    fn offered(graph: &Graph<Nodes>) -> Vec<&'static str> {
        QUICK_ACTIONS.iter().filter(|a| run(a.name, graph).is_some()).map(|a| a.name).collect()
    }

    #[test]
    fn the_standard_graph_records_it_all_from_real_devices() {
        let empty = Graph::default();
        assert_eq!(offered(&empty), vec!["Standard LTX graph v0.5"]);
        let change = run("Standard LTX graph v0.5", &empty).unwrap();
        assert!(change.arrange, "a new graph is laid out");
        assert!(change.summary.starts_with("added Acquisition Board, "), "{}", change.summary);
        assert_eq!(change.open.len(), 4, "the four viewers");
        let graph = change.graph;
        let resolved = graph.resolve();
        assert!(graph.iter().all(|(id, e)| e.node.warnings(&resolved[&id].input_streams).is_empty()), "all connected");
        let ltx = graph.iter().find(|(_, e)| e.node.info().name == "Write LTX").unwrap().0;
        let into = |i| graph.edges().iter().any(|e| e.target() == Port { node: ltx, role: Input, index: i });
        assert!((0..4).all(into), "Write LTX records spikes, EEG, positions and digital inputs");
        assert!(graph.iter().any(|(_, e)| matches!(&e.node, Nodes::AcquisitionBoard(b)
            if b.source == real_board())));
        // Then: disabling positions, and using synthetic data, are offered.
        assert_eq!(offered(&graph), vec!["Disable position tracking", "Use synthetic data"]);
    }

    #[test]
    fn positions_are_disabled_and_enabled() {
        let graph = run("Standard LTX graph v0.5", &Graph::default()).unwrap().graph;
        let disabled = run("Disable position tracking", &graph).unwrap().graph;
        assert_eq!(position_nodes(&disabled).len(), 3);
        assert!(position_nodes(&disabled).iter().all(|id| disabled.get(*id).unwrap().disabled));
        assert!(run("Disable position tracking", &disabled).is_none());
        // Any of it disabled, enabling's offered (and disabling isn't).
        let mut partly = graph.clone();
        partly.get_mut(position_nodes(&graph)[0]).unwrap().disabled = true;
        assert!(
            run("Enable position tracking", &partly).is_some() && run("Disable position tracking", &partly).is_none()
        );
        let enabled = run("Enable position tracking", &disabled).unwrap().graph;
        assert!(position_nodes(&enabled).iter().all(|id| !enabled.get(*id).unwrap().disabled));
    }

    #[test]
    fn synthetic_and_real_data_swap() {
        let example = crate::example_graph();
        // The example has all the standard graph has (and more), from the synthetic data.
        assert_eq!(offered(&example), vec!["Disable position tracking", "Use real devices"]);
        let real = run("Use real devices", &example).unwrap().graph;
        assert!(run("Use real devices", &real).is_none());
        let synthetic = run("Use synthetic data", &real).unwrap().graph;
        let sources = |graph: &Graph<Nodes>| {
            graph
                .iter()
                .filter_map(|(_, e)| match &e.node {
                    Nodes::AcquisitionBoard(b) => Some(format!("{:?}", b.source)),
                    Nodes::Camera(c) => Some(format!("{:?}", c.source)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(sources(&synthetic).iter().all(|s| s.starts_with("Fake")), "{:?}", sources(&synthetic));
    }
}
