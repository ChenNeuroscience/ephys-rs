//! An example of a patch, to copy when adding one; this module itself holds nothing. A patch is a file of its own with
//! a `PATCH` constant, listed at the end of the set's `PATCHES` in `mod.rs`. The patch's function receives the incoming
//! config (`IncomingConfig`), shaped as a project's JSON (see `ephys_rs::config_patching`), and converts what was
//! written before into what's written now.
//!
//! A build's patch is made with `Patch::build`, which takes two versions. The first is the build's version from which
//! config is written in the new form: the app only calls the function for config written before it, so the function
//! doesn't check versions itself. The second is the ephys-rs version the build uses as the patch is written (in
//! Cargo.toml), which places the patch among the core's and standard components' patches. Both are written as
//! literals, and never change. For example, to give the VR Computer a new setting from version 0.1008.0, written while
//! the build used ephys-rs 0.230.0:
//!
//! ```ignore
//! use ephys_rs::config_patching::{IncomingConfig, Patch};
//!
//! pub const PATCH: Patch = Patch::build("0.1008.0", "0.230.0", patch);
//!
//! fn patch(config: &mut IncomingConfig) {
//!     for node in config.nodes().filter(|node| node["type"] == "VrComputer") {
//!         node["settings"]["timeout"] = 5.into();
//!     }
//! }
//! ```
