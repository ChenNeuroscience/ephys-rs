//! The node types this program is built with.
//!
//! To add a type, write a module with a settings struct implementing `engine::Node`, and list it in
//! `node_set!` below. The order there is the order in the library panel.

mod acquisition_board;
mod band_pass;
mod camera;
mod gain;
mod lfp_viewer;
mod position_detector;
mod preview;
mod spike_detector;
mod spike_viewer;
mod tracker_view;
mod video_viewer;
mod writer;

use eframe::egui;

use crate::edges::{Edges, Signal};
use crate::engine::{EdgeSet, Graph, NodeSet, Port, Role};
use crate::node_set;

node_set! {
    edges: Edges;
    pub enum Nodes {
        AcquisitionBoard(acquisition_board::AcquisitionBoard),
        Camera(camera::Camera),
        BandPassFilter(band_pass::BandPassFilter),
        Gain(gain::Gain),
        SpikeDetector(spike_detector::SpikeDetector),
        PositionDetector(position_detector::PositionDetector),
        LfpViewer(lfp_viewer::LfpViewer),
        SpikeViewer(spike_viewer::SpikeViewer),
        VideoViewer(video_viewer::VideoViewer),
        TrackerView(tracker_view::TrackerView),
        Writer(writer::Writer),
    }
}

/// A small rig: an acquisition board filtered into a spike detector, and a camera tracking LEDs, all viewed and
/// recorded.
pub fn example() -> Graph<Nodes> {
    let mut graph = Graph::default();
    let mut add = |name: &str, pos: [f32; 2]| {
        let index = Nodes::TYPES.iter().position(|t| t.name == name).expect("node type");
        graph.add(Nodes::create(index), pos)
    };
    let board = add("Acquisition Board", [0.0, 0.0]);
    let filter = add("Band-pass Filter", [260.0, 0.0]);
    let detector = add("Spike Detector", [520.0, 0.0]);
    let spikes = add("Spike Viewer", [780.0, 0.0]);
    let lfp = add("LFP Viewer", [780.0, 110.0]);
    let writer = add("Writer", [1040.0, 120.0]);
    let camera = add("Camera", [0.0, 320.0]);
    let positions = add("Position Detector", [400.0, 380.0]);
    let tracker = add("Tracker View", [780.0, 330.0]);
    let mut link = |from, from_role, from_index, to, to_role, to_index| {
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(from, from_role, from_index), port(to, to_role, to_index)).expect("example edge");
    };
    use Role::{Control, Input, Output, Setting};
    link(board, Output, 0, filter, Input, 0);
    link(filter, Output, 0, detector, Input, 0);
    link(filter, Output, 0, lfp, Input, 0);
    link(detector, Output, 0, spikes, Input, 0);
    link(board, Output, 0, writer, Input, 0);
    link(board, Output, 1, writer, Input, 1);
    link(detector, Output, 0, writer, Input, 2);
    link(camera, Output, 0, positions, Input, 0);
    link(camera, Output, 0, tracker, Input, 0);
    link(positions, Output, 0, tracker, Input, 1);
    link(camera, Output, 0, writer, Input, 3);
    link(positions, Output, 0, writer, Input, 4);
    // Viewers adjust nodes upstream of them: the spike viewer the detector's threshold, and the tracker view
    // the camera and the position detector.
    link(spikes, Control, 0, detector, Setting, 0);
    link(tracker, Control, 0, camera, Setting, 0);
    link(tracker, Control, 1, positions, Setting, 0);
    graph
}

/// Two-column grid of labelled settings.
fn settings_grid(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    named_settings_grid(ui, "settings", add);
}

/// A settings grid with its own id, for when there's more than one in the same place.
fn named_settings_grid(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id).num_columns(2).spacing([12.0, 6.0]).show(ui, add);
}

/// A row of `settings_grid`: a label and the widget it names.
fn setting(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> egui::Response) {
    let label = ui.label(label);
    add(ui).labelled_by(label.id);
    ui.end_row();
}

fn combo<T: PartialEq + Copy>(ui: &mut egui::Ui, salt: &str, value: &mut T, options: &[(T, &str)]) -> egui::Response {
    let selected = options.iter().find(|(v, _)| v == value).map_or("?", |(_, name)| name);
    egui::ComboBox::from_id_salt(salt)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for (option, name) in options {
                ui.selectable_value(value, *option, *name);
            }
        })
        .response
}

fn require_input(inputs: &[Option<Signal>], what: &str) -> Vec<String> {
    if inputs.iter().all(|i| i.is_none()) { vec![format!("Connect {what}")] } else { Vec::new() }
}

/// Shows the incoming signals, for settings panels of nodes that pass them on.
fn input_summary(ui: &mut egui::Ui, inputs: &[Option<Signal>]) {
    for signal in inputs {
        match signal {
            Some(s) => ui.weak(format!("Input: {}", s.summary())),
            None => ui.weak("Input: not connected"),
        };
    }
}
