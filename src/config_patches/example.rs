//! An example of a patch, to copy when adding one; this module itself holds nothing. A patch is a file of its own with
//! a `PATCH` constant, listed at the end of the set's `PATCHES` in `mod.rs`. The patch's function receives the incoming
//! config (`IncomingConfig`), shaped as a project's JSON (see `crate::config_patching`), and converts what was written
//! before into what's written now. The app only calls the function for config written before the version given in
//! `Patch::new`, so the function doesn't check versions itself. For example, to move the layout under a key of its own
//! from version 0.159.0:
//!
//! ```ignore
//! use crate::config_patching::{IncomingConfig, Patch};
//!
//! pub const PATCH: Patch = Patch::new("0.159.0", patch);
//!
//! fn patch(config: &mut IncomingConfig) {
//!     if let Some(layout) = config.json.as_object_mut().and_then(|p| p.remove("layout")) {
//!         config.json["windows"] = serde_json::json!({ "layout": layout });
//!     }
//! }
//! ```
