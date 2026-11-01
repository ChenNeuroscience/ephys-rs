//! The standard components, grouped by what they're for: electrophysiology (`ephys`), spikes (`spikes`) and
//! video tracking (`tracking`), each with its edge types and node types; writers, which record them (`recording`);
//! annotation types (`annotations`); and themes (`themes`). main.rs declares which of them the program is
//! built with, and the names it gives the edge enums (`crate::DataStream`, `crate::Setting`...) are the names the
//! node types use.

pub mod annotations;
pub mod ephys;
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
