//! The standard components, grouped by what they're for: electrophysiology (`ephys`), spikes (`spikes`) and
//! video tracking (`tracking`), each with its edge types and node types; alarms on them all (`alarms`); writers, which
//! record them (`recording`); annotation types (`annotations`); and themes (`themes`). A program declares which of them it's built with; the
//! edge enums they use (`crate::DataStream`, `crate::Setting`...) are in lib.rs.

pub mod alarms;
pub mod annotations;
pub mod auto_detect;
pub mod base_directories;
pub mod ephys;
pub mod pool;
mod preview;
pub mod recording;
pub mod spikes;
pub mod themes;
pub mod tracking;
mod widgets;

/// The most samples (or frames) of a data stream at `rate` that fall in `duration` seconds.
fn per_pass(rate: f32, duration: f64) -> usize {
    (rate as f64 * duration).ceil() as usize + 1
}
