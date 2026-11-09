//! chen-neuro's quick actions, offered in the basic view:
//!
//! - The standard LTX graph (v0.5): an Open Ephys acquisition board (generation 1 or 2) giving spikes, EEG and digital
//!   inputs, and a FLIR camera giving positions, all viewed, and recorded by Write LTX. It reuses what the graph
//!   already has, adding only what's missing (and connecting only ports that are free), so it's offered until the
//!   graph has it all. It arranges the panes: the tracker view in a column at the left (with home as a tab behind
//!   it); the spike viewer filling the rest above (with any panes open before as tabs behind it), and the LFP viewer
//!   below that (with the TTL viewer and the alarms as tabs behind it).
//! - Swapping the positions' source between the camera (with its position detector) and the VR Computer. What's unused
//!   is disabled, and whatever took the positions, their TTLs and the settings (the tracker view among them, which shows
//!   positions without video) takes them from the other, as an input has the one edge. The VR Computer's added the first
//!   time, and from then on kept, disabled, with no edges; likewise a Log Viewer, showing its log lines.
//! - Using synthetic data (the fake board and camera in `synthetic_streams`), while any board or camera is real; and
//!   real devices, while any is synthetic.

use super::{NodeType, Nodes};
use ephys_rs::engine::{Graph, NodeId, NodesEnum, Port, PortRole};
use ephys_rs::layout::{Pane, WindowState};
use ephys_rs::quick_action::{Arrangement, QuickAction, QuickChange, QuickState, Tab};
use ephys_rs::standard::alarms::processor::AlarmProcessor;
use ephys_rs::standard::alarms::{AlarmKind, Source};
use ephys_rs::standard::ephys::band_pass::BandPassFilter;
use ephys_rs::standard::ephys::board::{self, node::AcquisitionBoard, opal_kelly::OpalKellyKind};
use ephys_rs::standard::ephys::downsample::Downsample;
use ephys_rs::standard::tracking::camera::{self, node::Camera};

use super::vr_computer::VrComputer;

pub const QUICK_ACTIONS: &[QuickAction<Nodes>] = &[
    QuickAction {
        name: "Standard LTX graph v0.5",
        description: "An Open Ephys acquisition board (gen 1/2) and a FLIR camera: spikes, EEG, digital inputs and \
                      positions, viewed and recorded by Write LTX",
        run: standard,
    },
    QuickAction {
        name: "Use VR computer",
        description: "Positions, TTLs, log lines and settings from the VR computer, keeping the camera and position \
                      detector disabled",
        run: use_vr,
    },
    QuickAction {
        name: "Use camera",
        description: "Positions from the camera and position detector, keeping the VR computer disabled",
        run: use_camera,
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

    /// The first node of type `node_type`, or a new one, after `after` (if given).
    fn upsert(&mut self, node_type: NodeType, after: Option<NodeId>) -> NodeId {
        let found = self.graph.iter().find(|(_, e)| e.node.node_type() == node_type).map(|(id, _)| id);
        found.unwrap_or_else(|| self.add(Nodes::create(node_type), after))
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
    fn link(&mut self, from: (NodeId, PortRole, usize), to: (NodeId, PortRole, usize)) {
        let (from, to) = (port(from), port(to));
        let free = match to.role {
            PortRole::Input => !self.linked(to),
            _ => !self.linked(from),
        };
        if free && self.graph.connect(from, to).is_ok() {
            self.changed = true;
            self.connected += 1;
        }
    }

    /// The change, opening `windows` (those not open already, where they'd go: see `Layout::open`); nothing if
    /// nothing's changed (the graph already does it all).
    fn finish(self, state: &QuickState<Nodes>, windows: &[NodeId]) -> Option<QuickChange<Nodes>> {
        let mut layout = state.layout.clone();
        let open = layout.open_windows();
        for &window in windows.iter().filter(|w| !open.contains(w)) {
            layout.open(window, None);
        }
        let mut done = Vec::new();
        if !self.added.is_empty() {
            done.push(format!("added {}", self.added.join(", ")));
        }
        if self.connected > 0 {
            done.push(format!("connected {}", self.connected));
        }
        let summary = done.join("; ");
        self.changed.then_some(QuickChange { graph: self.graph, layout, arrange: false, summary, base_directory: None })
    }
}

fn port((node, role, index): (NodeId, PortRole, usize)) -> Port {
    Port { node, role, index }
}

use PortRole::{Control, Input, Output, Setting};

/// A real board: the Open Ephys acquisition board, generation 1 or 2.
fn real_board() -> board::Source {
    board::Source::OpalKelly { board: OpalKellyKind::OpenEphys }
}

/// A real camera: the first FLIR camera found.
fn real_camera() -> camera::Source {
    camera::Source::Spinnaker { serial: String::new() }
}

/// Board → Reference → Band-pass (spikes' band) → Spike Detector → Spike Gain → Spike Viewer (setting the
/// detector's thresholds and the gain's gains); Board → Channel Selector → Band-pass (LFP's band) → LFP Viewer
/// (choosing the selector's channels, which only the LFP branch is, the Continuous Gain's gains, and the spikes' Reference's
/// reference channels, and showing the reference), with a Spectrum of it too; the board's digital
/// inputs → TTL Viewer; Camera → Position Detector → Tracker View (setting the camera's and detector's settings).
/// Write LTX records the spikes (at their gains), the LFP branch's data as EEG (the channels selected, band-passed,
/// downsampled to 5 kHz, through a Continuous Gain), the positions (with the detector's settings), the digital inputs and the timing LED, if
/// there is one. An Alarm Processor (in the basic view, for its settings) watches the spikes, the LFP branch and the
/// positions, with the timing LED connected to add an alarm for, for an Alarm Viewer with switches for it. New boards and cameras are real. Recordings go in a directory for each subject, day and trial.
fn standard(state: &QuickState<Nodes>) -> Option<QuickChange<Nodes>> {
    let mut change = Change::new(state.graph);
    let empty = state.graph.iter().next().is_none();
    let board = match change.find(|n| matches!(n, Nodes::AcquisitionBoard(_))) {
        Some(board) => board,
        None => {
            change.add(Nodes::AcquisitionBoard(AcquisitionBoard { source: real_board(), ..Default::default() }), None)
        }
    };
    let reference = change.upsert(NodeType::Reference, Some(board));
    change.link((board, Output, 0), (reference, Input, 0));

    // Spikes.
    let band = match change.find(|n| matches!(n, Nodes::BandPassFilter(f) if f.low_hz >= 100.0)) {
        Some(band) => band,
        None => change.add(Nodes::BandPassFilter(Default::default()), Some(reference)),
    };
    change.link((reference, Output, 0), (band, Input, 0));
    let detector = change.upsert(NodeType::SpikeDetector, Some(band));
    change.link((band, Output, 0), (detector, Input, 0));
    let gain = change.upsert(NodeType::SpikeGain, Some(detector));
    change.link((detector, Output, 0), (gain, Input, 0));
    let spike_viewer = change.upsert(NodeType::SpikeViewer, Some(gain));
    change.link((gain, Output, 0), (spike_viewer, Input, 0));
    change.link((spike_viewer, Control, 0), (detector, Setting, 0));
    change.link((spike_viewer, Control, 1), (gain, Setting, 0));

    // EEG: only some channels, chosen in the LFP Viewer.
    let selector = change.upsert(NodeType::ChannelSelector, Some(board));
    change.link((board, Output, 0), (selector, Input, 0));
    let lfp = match change.find(|n| matches!(n, Nodes::BandPassFilter(f) if f.high_hz <= 500.0)) {
        Some(lfp) => lfp,
        None => {
            let filter = BandPassFilter { low_hz: 1.0, high_hz: 300.0, ..Default::default() };
            change.add(Nodes::BandPassFilter(filter), Some(selector))
        }
    };
    change.link((selector, Output, 0), (lfp, Input, 0));
    let spectrum = change.upsert(NodeType::Spectrum, Some(lfp));
    change.link((lfp, Output, 0), (spectrum, Input, 0));
    let lfp_viewer = change.upsert(NodeType::LfpViewer, Some(spectrum));
    change.link((lfp, Output, 0), (lfp_viewer, Input, 0));
    change.link((spectrum, Output, 0), (lfp_viewer, Input, 1));
    change.link((lfp_viewer, Control, 1), (selector, Setting, 0));

    // Digital inputs.
    let ttl_viewer = change.upsert(NodeType::TtlViewer, Some(board));
    change.link((board, Output, 1), (ttl_viewer, Input, 0));

    // Positions.
    let camera = match change.find(|n| matches!(n, Nodes::Camera(_))) {
        Some(camera) => camera,
        None => change.add(Nodes::Camera(Camera { source: real_camera(), ..Default::default() }), None),
    };
    let positions = change.upsert(NodeType::PositionDetector, Some(camera));
    change.link((camera, Output, 0), (positions, Input, 0));
    let tracker = change.upsert(NodeType::TrackerView, Some(positions));
    change.link((camera, Output, 0), (tracker, Input, 0));
    change.link((positions, Output, 0), (tracker, Input, 1));
    change.link((camera, Output, 1), (tracker, Input, 2));
    change.link((tracker, Control, 0), (camera, Setting, 0));
    change.link((tracker, Control, 1), (positions, Setting, 0));

    // Recording it all.
    let ltx = change.upsert(NodeType::WriteLtx, Some(gain));
    change.link((gain, Output, 0), (ltx, Input, 0));
    // The EEG, at LTX's 5 kHz.
    let downsample = match change.find(|n| matches!(n, Nodes::Downsample(d) if d.rate_hz == 5000.0)) {
        Some(downsample) => downsample,
        None => change.add(Nodes::Downsample(Downsample { rate_hz: 5000.0 }), Some(lfp)),
    };
    change.link((lfp, Output, 0), (downsample, Input, 0));
    let continuous_gain = change.upsert(NodeType::ContinuousGain, Some(downsample));
    change.link((downsample, Output, 0), (continuous_gain, Input, 0));
    change.link((continuous_gain, Output, 0), (ltx, Input, 1));
    change.link((lfp_viewer, Control, 0), (continuous_gain, Setting, 0));
    change.link((lfp_viewer, Control, 2), (reference, Setting, 0));
    change.link((reference, Output, 1), (lfp_viewer, Input, 2));
    change.link((positions, Output, 0), (ltx, Input, 2));
    change.link((board, Output, 1), (ltx, Input, 3));
    change.link((positions, Output, 1), (ltx, Input, 4));
    change.link((ltx, Control, 0), (positions, Setting, 0));
    change.link((ltx, Control, 1), (gain, Setting, 0));
    change.link((ltx, Control, 2), (selector, Setting, 0));
    change.link((ltx, Control, 3), (continuous_gain, Setting, 0));

    // Alarms: on the spikes, the LFP branch and the positions, with the timing LED there to add one for.
    let alarm_processor = match change.find(|n| matches!(n, Nodes::AlarmProcessor(_))) {
        Some(alarm_processor) => alarm_processor,
        None => {
            let mut node = AlarmProcessor::default();
            // Not the TTLs': what to expect of them depends on the rig. They're connected, for adding one.
            for kind in AlarmKind::library().into_iter().filter(|kind| kind.source() != Source::Ttl) {
                node.alarms.add(kind);
            }
            let added = change.add(Nodes::AlarmProcessor(node), Some(gain));
            // Its window has the alarms' settings.
            if let Some(entry) = change.graph.get_mut(added) {
                entry.show_in_basic_override = Some(true);
            }
            added
        }
    };
    change.link((gain, Output, 0), (alarm_processor, Input, 0));
    change.link((lfp, Output, 0), (alarm_processor, Input, 1));
    change.link((positions, Output, 0), (alarm_processor, Input, 2));
    change.link((positions, Output, 1), (alarm_processor, Input, 3));
    let alarm_viewer = change.upsert(NodeType::AlarmViewer, Some(alarm_processor));
    change.link((alarm_processor, Output, 0), (alarm_viewer, Input, 0));
    change.link((alarm_viewer, Control, 0), (alarm_processor, Setting, 0));

    let mut result = change.finish(state, &[spike_viewer, lfp_viewer, ttl_viewer, tracker, alarm_viewer])?;
    // A new graph, laid out tidily; recording each subject's trials.
    result.arrange = empty;
    result.base_directory = Some("Subject, date and trial");
    // The default layout: the tracker (with home behind it) on the left, the spikes and the EEG (with the TTLs and the
    // alarms) on the right; the panes there were before behind the spikes.
    result.layout.arrange(&Arrangement::default_layout(
        Arrangement::Tabs { tabs: vec![Tab::Home, Tab::Window(tracker)], active: 1 },
        Arrangement::tabs(vec![Tab::Window(spike_viewer), Tab::Others]),
        Arrangement::tabs(vec![Tab::Window(lfp_viewer), Tab::Window(ttl_viewer), Tab::Window(alarm_viewer)]),
    ));
    Some(result)
}

/// The nodes finding positions in video: cameras and position detectors.
fn position_nodes(graph: &Graph<Nodes>) -> Vec<NodeId> {
    graph
        .iter()
        .filter(|(_, e)| matches!(e.node, Nodes::Camera(_) | Nodes::PositionDetector(_)))
        .map(|(id, _)| id)
        .collect()
}

/// Whatever takes `from`'s positions, its TTLs and its settings takes `to`'s instead. (The outputs' order is the same on
/// the VR Computer and the position detector.)
fn move_positions(graph: &mut Graph<Nodes>, from: NodeId, to: NodeId) {
    for index in 0..2 {
        let targets: Vec<Port> = graph.edges_from(port((from, Output, index))).map(|e| e.target()).collect();
        for target in targets {
            let _ = graph.connect(port((to, Output, index)), target);
        }
    }
    let setting = port((from, Setting, 0));
    let controls: Vec<Port> = graph.edges().iter().filter(|e| e.target() == setting).map(|e| e.source()).collect();
    for control in controls {
        let _ = graph.connect(control, port((to, Setting, 0)));
    }
}

/// Sets whether each of `ids` is disabled.
fn set_disabled(graph: &mut Graph<Nodes>, ids: &[NodeId], disabled: bool) {
    for id in ids {
        if let Some(entry) = graph.get_mut(*id) {
            entry.disabled = disabled;
        }
    }
}

/// The VR Computer that's in use, if there is one.
fn live_vr(graph: &Graph<Nodes>) -> Option<NodeId> {
    let live = |e: &ephys_rs::engine::NodeEntry<Nodes>| matches!(e.node, Nodes::VrComputer(_)) && !e.disabled;
    graph.iter().find(|(_, e)| live(e)).map(|(id, _)| id)
}

/// The VR Computer instead of the camera, adding it if need be, unless it's in use (or there's no camera to swap).
fn use_vr(state: &QuickState<Nodes>) -> Option<QuickChange<Nodes>> {
    let mut change = Change::new(state.graph);
    let detector = change.find(|n| matches!(n, Nodes::PositionDetector(_)))?;
    if live_vr(state.graph).is_some() {
        return None;
    }
    let vr = change.find(|n| matches!(n, Nodes::VrComputer(_))).unwrap_or_else(|| {
        let added = change.add(Nodes::VrComputer(VrComputer::default()), None);
        // Its card has the address to connect to.
        if let Some(entry) = change.graph.get_mut(added) {
            entry.show_in_basic_override = Some(true);
        }
        added
    });
    let tracking = position_nodes(&change.graph);
    move_positions(&mut change.graph, detector, vr);
    set_disabled(&mut change.graph, &tracking, true);
    set_disabled(&mut change.graph, &[vr], false);
    if let Some(ltx) = change.find(|n| matches!(n, Nodes::WriteLtx(_))) {
        change.link((vr, Output, 2), (ltx, Input, 5));
    }
    let log_viewer = change.upsert(NodeType::LogViewer, Some(vr));
    set_disabled(&mut change.graph, &[log_viewer], false);
    change.link((vr, Output, 2), (log_viewer, Input, 0));
    change.changed = true;
    let ttl_viewer = change.find(|n| matches!(n, Nodes::TtlViewer(_)));
    let mut result = change.finish(state, &[])?;
    // The first time, with the other small viewers, behind what's in front there; after that, where it was.
    let beside = ttl_viewer.and_then(|id| result.layout.group_of(Pane::Node(id)));
    match beside.filter(|_| result.layout.window(log_viewer) == WindowState::Closed) {
        Some(group) => result.layout.add_tab(group, Pane::Node(log_viewer)),
        None => result.layout.open(log_viewer, None),
    }
    result.summary = "using the VR computer".into();
    Some(result)
}

/// The camera instead of the VR Computer, which is kept, disabled, with no edges; if it isn't in use already.
fn use_camera(state: &QuickState<Nodes>) -> Option<QuickChange<Nodes>> {
    let mut change = Change::new(state.graph);
    let detector = change.find(|n| matches!(n, Nodes::PositionDetector(_)))?;
    let tracking = position_nodes(&change.graph);
    let vr = live_vr(state.graph);
    let mut log_viewers = Vec::new();
    let in_use = tracking.iter().all(|id| state.graph.get(*id).is_some_and(|e| !e.disabled));
    if vr.is_none() && in_use {
        return None;
    }
    if let Some(vr) = vr {
        let fed = change.graph.edges_from(port((vr, Output, 2))).map(|e| e.to.node);
        let is_viewer = |id: &NodeId| matches!(state.graph.get(*id).map(|e| &e.node), Some(Nodes::LogViewer(_)));
        log_viewers = fed.filter(is_viewer).collect();
        move_positions(&mut change.graph, vr, detector);
        let edges = change.graph.edges().iter().filter(|e| e.from.node == vr || e.to.node == vr);
        for edge in edges.copied().collect::<Vec<_>>() {
            change.graph.disconnect(edge);
        }
        set_disabled(&mut change.graph, &[vr], true);
        set_disabled(&mut change.graph, &log_viewers, true);
    }
    set_disabled(&mut change.graph, &tracking, false);
    change.changed = true;
    let mut result = change.finish(state, &[])?;
    for log_viewer in log_viewers {
        result.layout.close(log_viewer);
    }
    result.summary = "using the camera".into();
    Some(result)
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
        layout: state.layout.clone(),
        arrange: false,
        summary,
        base_directory: None,
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
    use super::*;
    use ephys_rs::quick_action::Layout;

    fn run(name: &str, graph: &Graph<Nodes>) -> Option<QuickChange<Nodes>> {
        let action = QUICK_ACTIONS.iter().find(|a| a.name == name).expect("action");
        (action.run)(&QuickState { graph, layout: &Layout::default() })
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
        assert_eq!(change.base_directory, Some("Subject, date and trial"));
        assert!(change.summary.starts_with("added Acquisition Board, "), "{}", change.summary);
        assert_eq!(change.layout.open_windows().len(), 5, "the five viewers");
        let graph = change.graph;
        let resolved = graph.resolve();
        assert!(graph.iter().all(|(id, e)| e.node.warnings(&resolved[&id].input_streams).is_empty()), "all connected");
        // The Channel Selector's only on the LFP branch: the spikes' starts at the board.
        let named = |name: &str| graph.iter().find(|(_, e)| e.label == name).unwrap().0;
        let feeding = |id| {
            graph.edges().iter().find(|e| e.target() == Port { node: id, role: Input, index: 0 }).unwrap().from.node
        };
        assert_eq!(feeding(named("Reference")), named("Acquisition Board"));
        let selector = named("Channel Selector");
        let fed: Vec<NodeId> = graph
            .edges()
            .iter()
            .filter(|e| e.from.node == selector && e.dimension == ephys_rs::engine::Dimension::Data)
            .map(|e| e.to.node)
            .collect();
        let lfp =
            graph.iter().find(|(_, e)| matches!(&e.node, Nodes::BandPassFilter(f) if f.high_hz <= 500.0)).unwrap().0;
        assert_eq!(fed, vec![lfp], "only the LFP branch");
        let downsample = named("Downsample");
        assert_eq!(feeding(named("Continuous Gain")), downsample, "the EEG written is at 5 kHz");
        assert_eq!(feeding(downsample), lfp, "the LFP branch's, after the selection");
        // The alarms watch all but the TTLs, with no timing LED to expect them from.
        let alarm_processor = named("Alarm Processor");
        let inputs = &resolved[&alarm_processor].input_streams;
        assert!(inputs.iter().all(Option::is_some), "all connected");
        let into = Port { node: alarm_processor, role: Input, index: 1 };
        assert!(graph.edges().iter().any(|e| e.target() == into && e.from.node == lfp), "the LFP, filtered");
        assert!(graph.get(alarm_processor).unwrap().shown_in_basic(), "for its settings");
        let Nodes::AlarmProcessor(alarms) = &graph.get(named("Alarm Processor")).unwrap().node else { panic!() };
        let sources: Vec<_> = alarms.alarms.list.iter().map(|a| a.kind.source()).collect();
        assert_eq!(sources, vec![Source::Spikes, Source::Lfp, Source::Positions]);
        assert!(alarms.alarms.all_on(), "none added just to be off");
        // The LFP Viewer sets the EEG's gains, the selection and the spikes' reference.
        let lfp_viewer = named("LFP Viewer");
        let controls = |port| {
            let from = ephys_rs::engine::PortRef { node: lfp_viewer, port };
            let settings = graph.edges().iter().filter(|e| e.dimension == ephys_rs::engine::Dimension::Settings);
            settings.into_iter().find(|e| e.from == from).map(|e| e.to.node)
        };
        assert_eq!(controls(0), Some(named("Continuous Gain")));
        assert_eq!(controls(1), Some(selector));
        assert_eq!(controls(2), Some(named("Reference")));
        let reference_shown = Port { node: lfp_viewer, role: Input, index: 2 };
        assert_eq!(graph.edge_into(reference_shown).map(|e| (e.from.node, e.from.port)), Some((named("Reference"), 1)));
        let ltx = graph.iter().find(|(_, e)| e.node.info().name == "Write LTX").unwrap().0;
        let into = |i| graph.edges().iter().any(|e| e.target() == Port { node: ltx, role: Input, index: i });
        assert!((0..4).all(into), "Write LTX records spikes, EEG, positions and digital inputs");
        // With the EEG's selection and gains.
        let ltx_controls = |port| {
            let from = ephys_rs::engine::PortRef { node: ltx, port };
            graph.edges().iter().find(|e| e.dimension == ephys_rs::engine::Dimension::Settings && e.from == from)
        };
        assert_eq!(ltx_controls(2).map(|e| e.to.node), Some(selector));
        assert_eq!(ltx_controls(3).map(|e| e.to.node), Some(named("Continuous Gain")));
        assert!(graph.iter().any(|(_, e)| matches!(&e.node, Nodes::AcquisitionBoard(b)
            if b.source == real_board())));
        // Then: the VR computer, and using synthetic data, are offered.
        assert_eq!(offered(&graph), vec!["Use VR computer", "Use synthetic data"]);
    }

    /// What feeds a port, by the label of the node.
    fn feeding(graph: &Graph<Nodes>, node: NodeId, role: PortRole, index: usize) -> Option<String> {
        let edge = graph.edge_into(Port { node, role, index })?;
        Some(graph.get(edge.from.node)?.label.clone())
    }

    fn named(graph: &Graph<Nodes>, name: &str) -> NodeId {
        graph.iter().find(|(_, e)| e.label == name).unwrap().0
    }

    #[test]
    fn the_positions_swap_between_the_camera_and_the_vr_computer() {
        let camera = run("Standard LTX graph v0.5", &Graph::default()).unwrap().graph;
        assert!(run("Use camera", &camera).is_none(), "it's in use");
        assert!(!camera.iter().any(|(_, e)| matches!(e.node, Nodes::VrComputer(_))), "not until it's asked for");

        let vr = run("Use VR computer", &camera).unwrap().graph;
        assert_eq!(offered(&vr), vec!["Use camera", "Use synthetic data"], "and not the standard graph again");
        assert!(position_nodes(&vr).iter().all(|id| vr.get(*id).unwrap().disabled));
        let (ltx, alarms, computer) =
            (named(&vr, "Write LTX"), named(&vr, "Alarm Processor"), named(&vr, "VR Computer"));
        assert!(!vr.get(computer).unwrap().disabled && vr.get(computer).unwrap().shown_in_basic());
        for (node, input) in [(ltx, 2), (ltx, 4), (ltx, 5), (alarms, 2), (alarms, 3)] {
            assert_eq!(feeding(&vr, node, Input, input).as_deref(), Some("VR Computer"), "input {input}");
        }
        let change = run("Use VR computer", &camera).unwrap();
        let log_viewer = named(&vr, "Log Viewer");
        assert_eq!(feeding(&vr, log_viewer, Input, 0).as_deref(), Some("VR Computer"));
        assert!(change.layout.open_windows().contains(&log_viewer), "its window's opened");
        let settings = vr.edges_from(port((ltx, Control, 0))).next().unwrap();
        assert_eq!(settings.to.node, computer, "the custom settings are recorded");
        let resolved = vr.resolve();
        let warned = |id| vr.get(id).unwrap().node.warnings(&resolved[&id].input_streams);
        assert!(warned(ltx).is_empty() && warned(alarms).is_empty(), "all connected");
        let tracker = named(&vr, "Tracker View");
        assert!(!vr.get(tracker).unwrap().disabled, "it shows positions without video");
        assert_eq!(feeding(&vr, tracker, Input, 1).as_deref(), Some("VR Computer"));
        assert_eq!(vr.edges_from(port((tracker, Control, 1))).next().unwrap().to.node, computer, "for the window");
        assert!(resolved[&tracker].input_streams[0].is_none(), "no video");
        assert!(warned(tracker).is_empty());

        let back = run("Use camera", &vr).unwrap().graph;
        assert_eq!(offered(&back), vec!["Use VR computer", "Use synthetic data"]);
        let computer_entry = back.get(computer).expect("kept");
        assert!(computer_entry.disabled);
        assert!(!back.edges().iter().any(|e| e.from.node == computer || e.to.node == computer), "no edges left");
        assert!(position_nodes(&back).iter().all(|id| !back.get(*id).unwrap().disabled));
        for (node, input) in [(ltx, 2), (ltx, 4), (alarms, 2), (alarms, 3)] {
            assert_eq!(feeding(&back, node, Input, input).as_deref(), Some("Position Detector"), "input {input}");
        }
        assert_eq!(feeding(&back, ltx, Input, 5), None, "no log without the VR computer");
        assert!(back.get(log_viewer).unwrap().disabled, "kept, like the VR computer");
        assert_eq!(feeding(&back, log_viewer, Input, 0), None);
        assert_eq!(feeding(&back, tracker, Input, 1).as_deref(), Some("Position Detector"));
        assert_eq!(feeding(&back, tracker, Input, 0).as_deref(), Some("Camera"), "and the video again");
        let detector = named(&back, "Position Detector");
        assert_eq!(back.edges_from(port((ltx, Control, 0))).next().unwrap().to.node, detector);
        // The same again uses the one there is.
        let again = run("Use VR computer", &back).unwrap().graph;
        assert_eq!(again.iter().filter(|(_, e)| matches!(e.node, Nodes::VrComputer(_))).count(), 1);
        assert_eq!(again.iter().filter(|(_, e)| matches!(e.node, Nodes::LogViewer(_))).count(), 1);
        assert!(!again.get(log_viewer).unwrap().disabled);
        assert_eq!(again.edges().len(), vr.edges().len());
    }

    #[test]
    fn the_camera_can_be_brought_back_if_any_of_it_was_disabled_by_hand() {
        let mut graph = run("Standard LTX graph v0.5", &Graph::default()).unwrap().graph;
        graph.get_mut(position_nodes(&graph)[0]).unwrap().disabled = true;
        let enabled = run("Use camera", &graph).unwrap().graph;
        assert!(position_nodes(&enabled).iter().all(|id| !enabled.get(*id).unwrap().disabled));
    }

    #[test]
    fn synthetic_and_real_data_swap() {
        let example = super::super::example_graph();
        // The example has all the standard graph has (and more), from the synthetic data.
        assert_eq!(offered(&example), vec!["Use VR computer", "Use real devices"]);
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
