//! ephys-rs: electrophysiology acquisition, processing and recording, built around a node graph.
//!
//! The core (engine, annotations, themes, GUI) and the standard components (edge, node and annotation types, and
//! themes). A program lists the parts it's made of, including any of its own, and runs them: see `core::Components`,
//! `core::gui::run`, and `examples/chen-neuro/` for one. The kinds of data and settings the standard components share are declared
//! here.

pub mod core;
pub mod standard_components;

use standard_components::alarms;
use standard_components::ephys::{self, reference, spectrum};
use standard_components::spikes;
use standard_components::tracking::{self, camera};

edge_types! {
    pub struct Edges;
    data_streams: pub enum DataStream, Kind, Buffer {
        Continuous(ephys::Continuous),
        Events(ephys::Events),
        Spikes(spikes::Spikes),
        Video(tracking::Video),
        Positions(tracking::Positions),
        CameraStats(camera::CameraStats),
        Spectra(spectrum::Spectra),
        AlarmStates(alarms::AlarmStates),
    }
    settings: pub enum Setting, SettingKind {
        Thresholds(spikes::Thresholds),
        Gains(spikes::Gains),
        ChannelSelection(ephys::ChannelSelection),
        Reference(reference::Reference),
        Camera(camera::CameraSettings),
        Detection(tracking::Detection),
        Alarms(alarms::Alarms),
    }
}

// The tests are of the whole program: chen-neuro's.
#[cfg(test)]
extern crate self as ephys_rs;
#[cfg(test)]
#[path = "../examples/chen-neuro/src/components/mod.rs"]
mod components;
#[cfg(test)]
use components::{AnnotationTypes, Nodes, example_graph};
