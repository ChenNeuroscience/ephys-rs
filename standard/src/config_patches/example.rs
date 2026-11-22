//! An example of a patch, to copy when adding one; this module itself holds nothing. A patch is a file of its own with
//! a `PATCH` constant, listed at the end of the set's `PATCHES` in `mod.rs`. The patch's function receives the incoming
//! config (`IncomingConfig`), shaped as a project's JSON (see `ephys_rs::config_patching`), and converts what was
//! written before into what's written now. The app only calls the function for config written before the version
//! given in `Patch::new`, so the function doesn't check versions itself. For example, to rename a Spike Detector
//! setting from version 0.159.0:
//!
//! ```ignore
//! use ephys_rs::config_patching::{IncomingConfig, Patch};
//!
//! pub const PATCH: Patch = Patch::new("0.159.0", patch);
//!
//! fn patch(config: &mut IncomingConfig) {
//!     for node in config.nodes().filter(|node| node["type"] == "SpikeDetector") {
//!         if let Some(old) = node["settings"].as_object_mut().and_then(|s| s.remove("threshold")) {
//!             node["settings"]["thresholds"] = old;
//!         }
//!     }
//! }
//! ```
