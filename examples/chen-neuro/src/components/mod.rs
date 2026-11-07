//! What chen-neuro's made of: the standard components, its quick actions and its base directory.

pub mod china;
pub mod quick_actions;
pub mod trials;
pub mod vr_computer;

use ephys_rs::core::base_directory::BaseDirectoryType;
use ephys_rs::core::formats::Layer;
use ephys_rs::core::quick_action::QuickAction;
use ephys_rs::core::theme::Theme;
use ephys_rs::core::{Components, build_version};
use ephys_rs::standard_components::alarms::{processor, viewer};
use ephys_rs::standard_components::annotations::{arrow, group_box, heading, image, markdown, sticky_note};
use ephys_rs::standard_components::base_directories::A_DIRECTORY;
use ephys_rs::standard_components::ephys::{
    band_pass, board, channel_selector, continuous_gain, lfp_viewer, reference, spectrum, ttl_viewer,
};
use ephys_rs::standard_components::logs::log_viewer;
use ephys_rs::standard_components::other::run_command;
use ephys_rs::standard_components::recording::{ltx, raw};
use ephys_rs::standard_components::spikes::{spike_detector, spike_gain, spike_viewer};
use ephys_rs::standard_components::themes::{DARK, LIGHT};
use ephys_rs::standard_components::tracking::{camera, position_detector, tracker_view, video_viewer};
use ephys_rs::{Edges, annotation_set, node_set};

// In the order they're offered in the add-node menu.
node_set! {
    edges: Edges;
    pub enum Nodes {
        AcquisitionBoard(board::node::AcquisitionBoard),
        Camera(camera::node::Camera),
        VrComputer(vr_computer::VrComputer),
        Reference(reference::Reference),
        ChannelSelector(channel_selector::ChannelSelector),
        BandPassFilter(band_pass::BandPassFilter),
        Spectrum(spectrum::Spectrum),
        ContinuousGain(continuous_gain::ContinuousGain),
        SpikeDetector(spike_detector::SpikeDetector),
        SpikeGain(spike_gain::SpikeGain),
        PositionDetector(position_detector::PositionDetector),
        AlarmProcessor(processor::AlarmProcessor),
        LfpViewer(lfp_viewer::LfpViewer),
        SpikeViewer(spike_viewer::SpikeViewer),
        TtlViewer(ttl_viewer::TtlViewer),
        LogViewer(log_viewer::LogViewer),
        VideoViewer(video_viewer::VideoViewer),
        TrackerView(tracker_view::TrackerView),
        AlarmViewer(viewer::AlarmViewer),
        WriteLtx(ltx::WriteLtx),
        WriteRaw(raw::WriteRaw),
        RunCommand(run_command::RunCommand),
    }
}

annotation_set! {
    pub enum AnnotationTypes {
        StickyNote(sticky_note::StickyNote),
        Markdown(markdown::Markdown),
        Image(image::Image),
        Heading(heading::Heading),
        Box(group_box::GroupBox),
        Arrow(arrow::Arrow),
    }
}

/// The first is the default.
pub const THEMES: &[Theme] = &[DARK, LIGHT, china::CHINA];

/// In the order they're offered: the first is the default.
pub const BASE_DIRECTORIES: &[BaseDirectoryType] = &[A_DIRECTORY, trials::TRIALS];

/// The program's parts, for the core.
#[cfg_attr(test, allow(dead_code))]
pub struct ChenNeuro;

impl Components for ChenNeuro {
    type Nodes = Nodes;
    type Annotations = AnnotationTypes;
    const THEMES: &'static [Theme] = THEMES;
    const QUICK_ACTIONS: &'static [QuickAction<Nodes>] = quick_actions::QUICK_ACTIONS;
    const BASE_DIRECTORIES: &'static [BaseDirectoryType] = BASE_DIRECTORIES;
    const FORMATS: &'static [Layer] = FORMATS;
    const ABOUT: &'static str = "The Chen lab's build, for its LTX rig:\n\n\
        - **Quick actions** for setting up the rig, in the basic view\n\
        - **Subject, date and trial**: recordings filed by subject and day, and numbered for you\n\
        - **VR Computer**: positions, TTLs, log lines and settings from a VR task, instead of a camera\n\
        - [LTX format](https://github.com/ChenNeuroscience/ltx-format) recording, in alpha testing";

    fn version() -> String {
        build_version("chen-neuro", env!("CARGO_PKG_VERSION"))
    }
}

/// The formats of what's saved: the standard components', and those of this build's own node types.
pub const FORMATS: &[Layer] =
    &[ephys_rs::standard_components::FORMATS, Layer { name: "chen-neuro", oldest: 0, steps: &[] }];

/// The graph in tests/rig.json, for tests.
#[cfg(test)]
pub fn example_graph() -> ephys_rs::core::engine::Graph<Nodes> {
    let file: serde_json::Value = serde_json::from_str(include_str!("../../../../tests/rig.json")).unwrap();
    serde_json::from_value(file["graph"].clone()).unwrap()
}
