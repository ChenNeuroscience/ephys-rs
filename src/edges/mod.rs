//! The edge types this program is built with.
//!
//! Signals describe what flows along data edges, and how a pass's worth is held; settings are the values that
//! settings edges let one node edit on another. To add a type, write a `Copy` struct implementing
//! `engine::EdgeType` (and for a signal, `engine::SignalType`) and list it below.

pub mod settings;
pub mod signals;

use crate::edge_types;

edge_types! {
    pub struct Edges;
    signals: pub enum Signal, Kind, Buffer {
        Continuous(signals::Continuous),
        Events(signals::Events),
        Spikes(signals::Spikes),
        Video(signals::Video),
    }
    settings: pub enum Setting, SettingKind {
        Threshold(settings::Threshold),
        Gain(settings::Gain),
    }
}
