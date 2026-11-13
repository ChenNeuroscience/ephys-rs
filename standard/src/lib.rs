//! ephys-rs-standard: the standard components of ephys-rs, which programs built on it pick from. They're grouped by
//! what they're for: electrophysiology (`ephys`), spikes (`spikes`) and video tracking (`tracking`), each with its edge
//! types and node types; alarms on them all (`alarms`); writers, which record them (`recording`); programs to run on
//! what's recorded (`other`); annotation types (`annotations`); and themes (`themes`). The kinds of data and settings
//! they share (`Edges`, `DataStream`, `Setting`...) are declared here.
//!
//! Its tests are the framework's, which are of the whole program, and build this source as a module, `standard` (see
//! the framework's src/lib.rs). So its code calls itself `crate::standard`, and its own test build is empty.

#![cfg(not(all(test, standard_crate)))]

#[allow(clippy::single_component_path_imports, unused_imports)]
use crate as standard;

pub mod alarms;

pub mod annotations;
pub mod auto_detect;
pub mod base_directories;
pub mod config_patches;
pub mod ephys;
pub mod logs;
pub mod medians;
pub mod other;
mod plots;
pub mod pool;
pub mod recording;
pub mod spikes;
pub mod themes;
pub mod tracking;
pub mod widgets;

use ephys::{reference, spectrum};
use other::{run_command, sync_positions};
use tracking::camera;

ephys_rs::edge_types! {
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
        LogLines(logs::LogLines),
    }
    settings: pub enum Setting, SettingKind {
        Thresholds(spikes::Thresholds),
        Gains(spikes::Gains),
        ChannelSelection(ephys::ChannelSelection),
        Reference(reference::Reference),
        Camera(camera::CameraSettings),
        Detection(tracking::Detection),
        Alarms(alarms::Alarms),
        Command(run_command::CommandSettings),
        Sync(sync_positions::SyncSettings),
    }
    triggers: pub enum Trigger, TriggerKind {
        AutoDetect(auto_detect::AutoDetect),
        RunOnFiles(run_command::RunOnFiles),
    }
}

/// The most samples (or frames) of a data stream at `rate` that fall in `duration` seconds.
fn per_pass(rate: f32, duration: f64) -> usize {
    (rate as f64 * duration).ceil() as usize + 1
}
