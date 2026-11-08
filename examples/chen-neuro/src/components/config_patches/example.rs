//! An example, of the shape a patch takes, to copy: it doesn't do anything. A patch is a script: it's given the versions
//! that saved something, and that, as a project's JSON (see `ephys_rs::core::config_patching`), and changes what was
//! saved before into what's saved now, if it needs to. Giving the VR Computer a setting it didn't have, from the version
//! after 0.1007.0, say:
//!
//! ```ignore
//! if saved_before(saved, "chen-neuro", "0.1008.0") {
//!     for node in nodes(project).filter(|node| node["type"] == "VrComputer") {
//!         node["settings"]["timeout"] = 5.into();
//!     }
//! }
//! ```

use serde_json::Value;

use ephys_rs::core::config_patching::SavedVersions;

pub fn patch(saved: &SavedVersions, project: &mut Value) {
    let _ = (saved, project);
}
