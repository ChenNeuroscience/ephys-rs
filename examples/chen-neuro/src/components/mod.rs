//! What chen-neuro's made of: the standard components, its quick actions and its base directory.

pub mod china;
pub mod config_patches;
pub mod quick_actions;
pub mod trials;
pub mod vr_computer;

use crate::standard::alarms::{processor, viewer};
use crate::standard::annotations::{arrow, group_box, heading, image, markdown, sticky_note};
use crate::standard::base_directories::A_DIRECTORY;
use crate::standard::ephys::{
    band_pass, board, channel_selector, continuous_gain, downsample, lfp_viewer, reference, spectrum, ttl_viewer,
};
use crate::standard::logs::log_viewer;
use crate::standard::other::{arduino, run_command, sync_positions, ttl_recording_control};
use crate::standard::recording::{ltx, raw};
use crate::standard::spikes::{spike_detector, spike_gain, spike_viewer};
use crate::standard::themes::{DARK, LIGHT};
use crate::standard::tracking::{camera, position_detector, tracker_view, video_viewer};
use ephys_rs::base_directory::BaseDirectoryType;
use ephys_rs::config_patching::PatchSet;
use ephys_rs::quick_action::QuickAction;
use ephys_rs::theme::Theme;
use ephys_rs::{Components, build_version};
use ephys_rs::{annotations_enum, nodes_enum};

// The edge types chen-neuro's nodes need: all the standard ones, as it has every standard node type. A build lists only
// those its nodes' ports carry (each node's `PortEdges`), and its own beside them.
ephys_rs::edge_types! {
    pub struct Edges;
    data_streams: pub enum DataStream, DataStreamKind, Buffer {
        Continuous(crate::standard::ephys::Continuous),
        Events(crate::standard::ephys::Events),
        Spikes(crate::standard::spikes::Spikes),
        Video(crate::standard::tracking::Video),
        Positions(crate::standard::tracking::Positions),
        CameraStats(crate::standard::tracking::camera::CameraStats),
        Spectra(crate::standard::ephys::spectrum::Spectra),
        AlarmStates(crate::standard::alarms::AlarmStates),
        LogLines(crate::standard::logs::LogLines),
    }
    values: pub enum PublishedValue, ValueKind {
        Thresholds(crate::standard::spikes::Thresholds),
        Gains(crate::standard::spikes::Gains),
        ChannelSelection(crate::standard::ephys::ChannelSelection),
        Reference(crate::standard::ephys::reference::Reference),
        Camera(crate::standard::tracking::camera::CameraConfig),
        Detection(crate::standard::tracking::Detection),
        Alarms(crate::standard::alarms::Alarms),
        FileInput(crate::standard::other::run_command::FileInput),
    }
    commands: pub enum Command, CommandKind {
        SetThresholds(crate::standard::spikes::SetThresholds),
        SetGains(crate::standard::spikes::SetGains),
        SetChannelSelection(crate::standard::ephys::SetChannelSelection),
        SetReference(crate::standard::ephys::reference::SetReference),
        SetCamera(crate::standard::tracking::camera::SetCamera),
        SetDetection(crate::standard::tracking::SetDetection),
        SetAlarms(crate::standard::alarms::SetAlarms),
        Detect(crate::standard::auto_detect::Detect),
        ProcessFiles(crate::standard::other::run_command::ProcessFiles),
    }
}

// In the order they're offered in the add-node menu.
nodes_enum! {
    edges: Edges;
    pub enum Nodes {
        AcquisitionBoard(board::node::AcquisitionBoard),
        Camera(camera::node::Camera),
        VrComputer(vr_computer::VrComputer),
        Reference(reference::Reference),
        ChannelSelector(channel_selector::ChannelSelector),
        BandPassFilter(band_pass::BandPassFilter),
        Downsample(downsample::Downsample),
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
        SyncPositions(sync_positions::SyncPositions),
        TtlRecordingControl(ttl_recording_control::TtlRecordingControl),
        Arduino(arduino::Arduino),
    }
    pub enum NodeType;
}

annotations_enum! {
    pub enum AnnotationTypes {
        StickyNote(sticky_note::StickyNote),
        Markdown(markdown::Markdown),
        Image(image::Image),
        Heading(heading::Heading),
        Box(group_box::GroupBox),
        Arrow(arrow::Arrow),
    }
    pub enum AnnotationKind;
}

/// The first is the default.
pub const THEMES: &[Theme] = &[DARK, LIGHT, china::CHINA];

/// In the order they're offered: the first is the default.
pub const BASE_DIRECTORIES: &[BaseDirectoryType] = &[A_DIRECTORY, trials::TRIALS];

/// The program's parts, for the core.
pub struct ChenNeuro;

impl Components for ChenNeuro {
    type Nodes = Nodes;
    type Annotations = AnnotationTypes;
    const THEMES: &'static [Theme] = THEMES;
    const QUICK_ACTIONS: &'static [QuickAction<Nodes>] = quick_actions::QUICK_ACTIONS;
    const BASE_DIRECTORIES: &'static [BaseDirectoryType] = BASE_DIRECTORIES;
    const CONFIG_PATCHES: &'static [PatchSet] = CONFIG_PATCHES;
    const ABOUT: &'static str = "The Chen lab's build, for its LTX rig:\n\n\
        - **Quick actions** for setting up the rig, in the basic view\n\
        - **Subject, date and trial**: recordings filed by subject and day, and numbered for you\n\
        - **VR Computer**: positions, TTLs, log lines and settings from a VR task, instead of a camera\n\
        - [LTX format](https://github.com/ChenNeuroscience/ltx-format) recording, in alpha testing";

    fn version() -> String {
        build_version("chen-neuro", env!("CARGO_PKG_VERSION"))
    }
}

/// The config patches of what's saved, in the order they're applied: the core's, the standard components', and this
/// build's own.
pub const CONFIG_PATCHES: &[PatchSet] =
    &[ephys_rs::config_patches::PATCHES, crate::standard::config_patches::PATCHES, config_patches::PATCHES];

/// The graph in tests/rig.json, for tests: saved before there were versions, so patched up to date, as the app does.
#[cfg(test)]
pub fn example_graph() -> ephys_rs::engine::Graph<Nodes> {
    let mut file: serde_json::Value = serde_json::from_str(include_str!("../../../../tests/rig.json")).unwrap();
    let patches = ephys_rs::config_patching::PatchSets::new(CONFIG_PATCHES);
    patches.apply_to_project(&ephys_rs::config_patching::Versions::new(), &mut file);
    serde_json::from_value(file["graph"].clone()).unwrap()
}
