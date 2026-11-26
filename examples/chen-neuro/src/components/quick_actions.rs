//! chen-neuro's quick actions, offered in the basic view, as files in `quick_actions/` (see `ephys_rs::quick_action`
//! for what's in one):
//!
//! - The standard LTX graph (v0.5): an Open Ephys acquisition board (generation 1 or 2) giving spikes, EEG and digital
//!   inputs, and a FLIR camera giving positions, all viewed, and recorded by Write LTX, which has Post-Process
//!   Positions LTX put the positions on the board's clock once it's done; an Arduino, for pin 8 to be high while
//!   acquiring (which has the rig's sketch pulse pin 6, for syncing); and an Alarm Processor watching the spikes, the
//!   LFP and the positions. It reuses what the graph already has, adding only what's missing (and connecting only
//!   ports that are free), so it's offered until the graph has it all. It arranges the panes: the tracker view in a
//!   column at the left (with home as a tab behind it); the spike viewer filling the rest above (with any panes open
//!   before as tabs behind it), and the Continuous Data Viewer below that (with the TTL viewer and the alarms as tabs
//!   behind it).
//! - Swapping the positions' source between the camera (with its position detector) and the VR Computer. What's unused
//!   is disabled, and the tracker view, Write LTX and the Alarm Processor take the positions, their TTLs and the
//!   settings from the other. The VR Computer's added the first time, and from then on kept, disabled, with no edges;
//!   likewise a Log Viewer, showing its log lines. The Arduino's for the camera, so it's disabled with it, and enabled
//!   again with it.
//! - Using synthetic data (the fake board and camera in `synthetic_streams`), while any board or camera is real; and
//!   real devices, while any is synthetic.

/// The quick actions, as their files have them, for tests: a build has none of its own (the release's zip has these
/// files, for people to add to their quick actions' folder).
#[cfg(test)]
pub const QUICK_ACTIONS: &[&str] = &[
    include_str!("../../quick_actions/standard-ltx-graph.quick-action.json"),
    include_str!("../../quick_actions/use-vr-computer.quick-action.json"),
    include_str!("../../quick_actions/use-camera.quick-action.json"),
    include_str!("../../quick_actions/use-synthetic-data.quick-action.json"),
    include_str!("../../quick_actions/use-real-devices.quick-action.json"),
];

#[cfg(test)]
mod tests {
    use ephys_rs::config_patching::PatchSets;
    use ephys_rs::engine::{Graph, NodeId, NodesEnum, PortAddress, PortRole};
    use ephys_rs::quick_action::{Layout, QuickAction, QuickChange, QuickState};

    use super::super::Nodes;
    use super::*;
    use crate::standard::alarms::Source;
    use crate::standard::ephys::board;
    use crate::standard::other::arduino::Arduino;
    use PortRole::{DataStreamIn, ValIn, ValInCmdOut};

    fn actions() -> Vec<QuickAction<Nodes>> {
        let patches = PatchSets::new(super::super::CONFIG_PATCHES);
        QUICK_ACTIONS.iter().map(|text| QuickAction::parse(text, &patches).unwrap()).collect()
    }

    fn run(name: &str, graph: &Graph<Nodes>) -> Option<QuickChange<Nodes>> {
        let action = actions().into_iter().find(|a| a.name == name).expect("action");
        action.run(&QuickState { graph, layout: &Layout::default() })
    }

    fn offered(graph: &Graph<Nodes>) -> Vec<String> {
        let state = QuickState { graph, layout: &Layout::default() };
        actions().into_iter().filter(|a| a.run(&state).is_some()).map(|a| a.name).collect()
    }

    fn port((node, role, port_idx): (NodeId, PortRole, usize)) -> PortAddress {
        PortAddress { node, role, port_idx }
    }

    /// The cameras and position detectors.
    fn position_nodes(graph: &Graph<Nodes>) -> Vec<NodeId> {
        let finding =
            |e: &ephys_rs::engine::NodeEntry<Nodes>| matches!(e.node, Nodes::Camera(_) | Nodes::PositionDetector(_));
        graph.iter().filter(|(_, e)| finding(e)).map(|(id, _)| id).collect()
    }

    #[test]
    fn each_reads_and_saves_as_it_was() {
        // Without patches: built into the tests, the build has ephys-rs's version, so would patch what it saved again.
        let patches = PatchSets::default();
        for action in actions() {
            let again = QuickAction::<Nodes>::parse(&action.to_text(&patches), &patches).unwrap();
            assert_eq!(again.to_text(&patches), action.to_text(&patches), "{}", action.name);
        }
    }

    #[test]
    fn the_standard_graph_records_it_all_from_real_devices() {
        let empty = Graph::default();
        assert_eq!(offered(&empty), vec!["Standard LTX graph v0.5"]);
        let change = run("Standard LTX graph v0.5", &empty).unwrap();
        assert!(change.arrange, "a new graph is laid out");
        assert_eq!(change.base_directory.as_deref(), Some("Subject, date and trial"));
        assert!(change.summary.starts_with("added Acquisition Board, "), "{}", change.summary);
        assert_eq!(change.layout.open_windows().len(), 5, "the five viewers");
        let graph = change.graph;
        let resolved = graph.resolve();
        assert!(graph.iter().all(|(id, e)| e.node.warnings(&resolved[&id].input_streams).is_empty()), "all connected");
        // The Channel Selector's only on the LFP branch: the spikes' starts at the board.
        let named = |name: &str| graph.iter().find(|(_, e)| e.label == name).unwrap().0;
        let feeding = |id| {
            graph
                .edges()
                .iter()
                .find(|e| e.to == PortAddress { node: id, role: DataStreamIn, port_idx: 0 })
                .unwrap()
                .from
                .node
        };
        assert_eq!(feeding(named("Reference")), named("Acquisition Board"));
        let selector = named("Channel Selector");
        let fed: Vec<NodeId> = graph
            .edges()
            .iter()
            .filter(|e| e.from.node == selector && e.carries_data_stream())
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
        let into = PortAddress { node: alarm_processor, role: DataStreamIn, port_idx: 1 };
        assert!(graph.edges().iter().any(|e| e.to == into && e.from.node == lfp), "the LFP, filtered");
        assert!(graph.get(alarm_processor).unwrap().shown_in_basic(), "for its settings");
        let Nodes::AlarmProcessor(alarms) = &graph.get(named("Alarm Processor")).unwrap().node else { panic!() };
        let sources: Vec<_> = alarms.alarms.list.iter().map(|a| a.kind.source()).collect();
        assert_eq!(sources, vec![Source::Spikes, Source::Lfp, Source::Positions]);
        assert!(alarms.alarms.all_on(), "none added just to be off");
        // The Continuous Data Viewer sets the EEG's gains, the selection and the spikes' reference.
        let continuous_viewer = named("Continuous Data Viewer");
        // A node's port it reads a value through, by the node's own numbering (see `val_in_address`).
        let reading = |node: NodeId, val_in_idx| {
            let ports = graph.get(node).unwrap().node.list_ports_val_cmd();
            let (role, port_idx) = ephys_rs::engine::val_in_address(&ports, val_in_idx).unwrap();
            PortAddress { node, role, port_idx }
        };
        let controls = |val_in_idx| {
            let from = reading(continuous_viewer, val_in_idx);
            graph.edges().iter().find(|e| e.from == from).map(|e| e.to.node)
        };
        assert_eq!(controls(0), Some(named("Continuous Gain")));
        assert_eq!(controls(1), Some(selector));
        assert_eq!(controls(2), Some(named("Reference")));
        let reference_shown = PortAddress { node: continuous_viewer, role: DataStreamIn, port_idx: 1 };
        assert_eq!(
            graph.edge_into(reference_shown).map(|e| (e.from.node, e.from.port_idx)),
            Some((named("Reference"), 1))
        );
        let ltx = graph.iter().find(|(_, e)| e.node.info().name == "Write LTX").unwrap().0;
        let into = |i| graph.edges().iter().any(|e| e.to == PortAddress { node: ltx, role: DataStreamIn, port_idx: i });
        assert!((0..4).all(into), "Write LTX records spikes, EEG, positions and digital inputs");
        // With the EEG's selection and gains.
        let ltx_controls = |val_in_idx| {
            let from = reading(ltx, val_in_idx);
            graph.edges().iter().find(|e| e.from == from)
        };
        assert_eq!(ltx_controls(2).map(|e| e.to.node), Some(selector));
        assert_eq!(ltx_controls(3).map(|e| e.to.node), Some(named("Continuous Gain")));
        assert_eq!(ltx_controls(4).map(|e| e.to.node), Some(named("Post-Process Positions LTX")), "after recording");
        let real_board = board::Source::OpalKelly { board: board::OpalKellyKind::OpenEphys };
        assert!(graph.iter().any(|(_, e)| matches!(&e.node, Nodes::AcquisitionBoard(b) if b.source == real_board)));
        let arduino = graph.get(named("Arduino")).unwrap();
        assert!(!arduino.disabled);
        assert!(matches!(&arduino.node, Nodes::Arduino(a) if *a == Arduino::default()), "pin 8 high while acquiring");
        // Then: the VR computer, and using synthetic data, are offered.
        assert_eq!(offered(&graph), vec!["Use VR computer", "Use synthetic data"]);
        // Done again on its own graph, it would change nothing.
        assert!(run("Standard LTX graph v0.5", &graph).is_none());
    }

    #[test]
    fn the_standard_graph_reuses_what_there_is() {
        let full = run("Standard LTX graph v0.5", &Graph::default()).unwrap().graph;
        let mut without = full.clone();
        let viewer = without.iter().find(|(_, e)| e.label == "Spike Viewer").unwrap().0;
        without.remove(viewer);
        let change = run("Standard LTX graph v0.5", &without).unwrap();
        assert_eq!(change.summary, "added Spike Viewer; connected 3");
        assert!(!change.arrange, "the rest stays where it is");
        assert_eq!(change.graph.iter().count(), full.iter().count());
        assert_eq!(change.graph.edges().len(), full.edges().len());
    }

    /// What feeds a port, by the label of the node.
    fn feeding(graph: &Graph<Nodes>, node: NodeId, role: PortRole, index: usize) -> Option<String> {
        let edge = graph.edge_into(PortAddress { node, role, port_idx: index })?;
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
        let arduino = named(&vr, "Arduino");
        assert!(vr.get(arduino).unwrap().disabled, "it's for the camera");
        let (ltx, alarms, computer) =
            (named(&vr, "Write LTX"), named(&vr, "Alarm Processor"), named(&vr, "VR Computer"));
        assert!(!vr.get(computer).unwrap().disabled && vr.get(computer).unwrap().shown_in_basic());
        for (node, input) in [(ltx, 2), (ltx, 4), (ltx, 5), (alarms, 2), (alarms, 3)] {
            assert_eq!(feeding(&vr, node, DataStreamIn, input).as_deref(), Some("VR Computer"), "input {input}");
        }
        let change = run("Use VR computer", &camera).unwrap();
        let log_viewer = named(&vr, "Log Viewer");
        assert_eq!(feeding(&vr, log_viewer, DataStreamIn, 0).as_deref(), Some("VR Computer"));
        assert!(change.layout.open_windows().contains(&log_viewer), "its window's opened");
        let settings = vr.edges_from(port((ltx, ValIn, 0))).next().unwrap();
        assert_eq!(settings.to.node, computer, "the custom settings are recorded");
        let resolved = vr.resolve();
        let warned = |id| vr.get(id).unwrap().node.warnings(&resolved[&id].input_streams);
        assert!(warned(ltx).is_empty() && warned(alarms).is_empty(), "all connected");
        let tracker = named(&vr, "Tracker View");
        assert!(!vr.get(tracker).unwrap().disabled, "it shows positions without video");
        assert_eq!(feeding(&vr, tracker, DataStreamIn, 1).as_deref(), Some("VR Computer"));
        assert_eq!(vr.edges_from(port((tracker, ValInCmdOut, 1))).next().unwrap().to.node, computer, "for the window");
        assert!(resolved[&tracker].input_streams[0].is_none(), "no video");
        assert!(warned(tracker).is_empty());

        let back = run("Use camera", &vr).unwrap().graph;
        assert_eq!(offered(&back), vec!["Use VR computer", "Use synthetic data"]);
        let computer_entry = back.get(computer).expect("kept");
        assert!(computer_entry.disabled);
        // Its edges are left in, disabled (a quick action never disconnects), or replaced by the detector's.
        let computers = back.edges().iter().filter(|e| e.from.node == computer || e.to.node == computer);
        assert!(computers.clone().all(|e| back.is_edge_disabled(e)), "its edges are disabled");
        assert!(position_nodes(&back).iter().all(|id| !back.get(*id).unwrap().disabled));
        assert!(!back.get(arduino).unwrap().disabled, "and with it, the Arduino");
        for (node, input) in [(ltx, 2), (ltx, 4), (alarms, 2), (alarms, 3)] {
            assert_eq!(
                feeding(&back, node, DataStreamIn, input).as_deref(),
                Some("Position Detector"),
                "input {input}"
            );
        }
        let log_into_ltx = back.edges().iter().find(|e| e.to.node == ltx && e.to.port_idx == 5).copied();
        assert!(log_into_ltx.is_none_or(|e| back.is_edge_disabled(&e)), "no log without the VR computer");
        assert!(back.get(log_viewer).unwrap().disabled, "kept, like the VR computer");
        let log_into_viewer = back.edges().iter().find(|e| e.to.node == log_viewer).copied();
        assert!(log_into_viewer.is_none_or(|e| back.is_edge_disabled(&e)));
        assert_eq!(feeding(&back, tracker, DataStreamIn, 1).as_deref(), Some("Position Detector"));
        assert_eq!(feeding(&back, tracker, DataStreamIn, 0).as_deref(), Some("Camera"), "and the video again");
        let detector = named(&back, "Position Detector");
        assert_eq!(back.edges_from(port((ltx, ValIn, 0))).next().unwrap().to.node, detector);
        // The same again uses the one there is.
        let again = run("Use VR computer", &back).unwrap().graph;
        assert_eq!(again.iter().filter(|(_, e)| matches!(e.node, Nodes::VrComputer(_))).count(), 1);
        assert_eq!(again.iter().filter(|(_, e)| matches!(e.node, Nodes::LogViewer(_))).count(), 1);
        assert!(!again.get(log_viewer).unwrap().disabled);
        let computers = again.edges().iter().filter(|e| e.from.node == computer || e.to.node == computer);
        assert!(computers.clone().all(|e| !again.is_edge_disabled(e)), "its edges enabled again");
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
