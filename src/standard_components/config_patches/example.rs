//! An example, of the shape a patch takes, to copy: it doesn't do anything. A patch is a script: it's given config coming in
//! (`IncomingConfig`): the versions that wrote it, and it, as a project's JSON (see `crate::core::config_patching`), and changes what was
//! written before into what's written now, if it needs to. Renaming a setting of the Spike Detector, from the version after
//! 0.158.0, say:
//!
//! ```ignore
//! if config.written_before("std", "0.159.0") {
//!     for node in config.nodes().filter(|node| node["type"] == "SpikeDetector") {
//!         if let Some(old) = node["settings"].as_object_mut().and_then(|s| s.remove("threshold")) {
//!             node["settings"]["thresholds"] = old;
//!         }
//!     }
//! }
//! ```

use crate::core::config_patching::IncomingConfig;

pub fn patch(config: &mut IncomingConfig) {
    let _ = config;
}
