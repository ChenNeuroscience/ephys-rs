//! The standard components, grouped by what they're for: electrophysiology (`ephys`), spikes (`spikes`) and
//! video tracking (`tracking`), each with its edge types and node types; alarms on them all (`alarms`); writers, which
//! record them (`recording`); programs to run on what's recorded (`other`); annotation types (`annotations`); and themes (`themes`). A program declares which of them it's built with; the
//! edge enums they use (`DataStream`, `Setting`...) are declared here, and re-exported at the crate's root.

pub mod alarms;

pub mod annotations;
pub mod auto_detect;
pub mod base_directories;
pub mod config_patches;
pub mod ephys;
pub mod logs;
pub mod other;
mod plots;
pub mod pool;
pub mod recording;
pub mod spikes;
pub mod themes;
pub mod tracking;
pub mod widgets;

use ephys::{reference, spectrum};
use other::run_command;
use tracking::camera;

crate::edge_types! {
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
