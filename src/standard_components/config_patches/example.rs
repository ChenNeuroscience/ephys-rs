//! An example, of the shape a patch takes, to copy: it doesn't do anything. A patch is a script: it's given the versions
//! that saved something, and that, as a project's JSON (see `crate::core::config_patching`), and changes what was
//! saved before into what's saved now, if it needs to. Renaming a setting of the Spike Detector, from the version after
//! 0.158.0, say:
//!
//! ```ignore
//! if saved_before(saved, "std", "0.159.0") {
//!     for node in nodes(project).filter(|node| node["type"] == "SpikeDetector") {
//!         if let Some(old) = node["settings"].as_object_mut().and_then(|s| s.remove("threshold")) {
//!             node["settings"]["thresholds"] = old;
//!         }
//!     }
//! }
//! ```

use serde_json::Value;

use crate::core::config_patching::SavedVersions;

pub fn patch(saved: &SavedVersions, project: &mut Value) {
    let _ = (saved, project);
}
