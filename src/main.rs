//! ephys-rs: electrophysiology acquisition, processing and recording, built around a node graph.
//!
//!     python3 synthetic_streams/fake_acquisition_board.py   # in other terminals: the example's board...
//!     python3 synthetic_streams/fake_camera.py              # ...and camera
//!     cargo run --release -- examples/rig.json             # the example graph (or any graph file, or a new one)
//!     cargo run --release                                  # an empty graph
//!
//! This is where the program's parts are put together: the core (engine, annotations, themes, GUI) with the
//! standard components (edge, node and annotation types, and themes), declared below.

// No console window alongside the app in Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core;
mod standard_components;

use standard_components::annotations::sticky_note;
use standard_components::ephys::{self, band_pass, board, gain, lfp_viewer, reference};
use standard_components::spikes::{self, spike_detector, spike_viewer};
use standard_components::tracking::{self, camera, position_detector, tracker_view, video_viewer};
use standard_components::writer;

edge_types! {
    pub struct Edges;
    data_streams: pub enum DataStream, Kind, Buffer {
        Continuous(ephys::Continuous),
        Events(ephys::Events),
        Spikes(spikes::Spikes),
        Video(tracking::Video),
        Positions(tracking::Positions),
        CameraStats(camera::CameraStats),
    }
    settings: pub enum Setting, SettingKind {
        Threshold(spikes::Threshold),
        Gain(ephys::Gain),
        Camera(camera::CameraSettings),
        Detection(tracking::Detection),
    }
}

// In the order they're offered in the add-node menu.
node_set! {
    edges: Edges;
    pub enum Nodes {
        AcquisitionBoard(board::node::AcquisitionBoard),
        Camera(camera::node::Camera),
        Reference(reference::Reference),
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

annotation_set! {
    pub enum AnnotationTypes {
        StickyNote(sticky_note::StickyNote),
    }
}

/// The program's parts, for the core.
struct Standard;

impl core::Components for Standard {
    type Nodes = Nodes;
    type Annotations = AnnotationTypes;
    const THEMES: &'static [core::theme::Theme] = standard_components::themes::THEMES;
}

/// The graph in examples/rig.json, for tests.
#[cfg(test)]
fn example_graph() -> core::engine::Graph<Nodes> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/rig.json");
    let file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    serde_json::from_value(file["graph"].clone()).unwrap()
}

fn main() -> eframe::Result {
    core::gui::run::<Standard>(std::env::args().nth(1).map(Into::into))
}
