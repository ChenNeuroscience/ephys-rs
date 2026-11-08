//! An example, of the shape a patch takes, to copy: it doesn't do anything. A patch decides for itself whether to, from
//! the versions that saved what it's given (see `ephys_rs::core::config_patching`), and what that is, and changes it as
//! JSON, from what was saved before to what's saved now. Giving the VR Computer a setting it didn't have, from the version
//! after 0.1006.0, say:
//!
//! ```ignore
//! if what == SavedPart::Node("VrComputer")
//!     && saved_before(saved, "chen-neuro", "0.1007.0")
//!     && let Some(node) = value.as_object_mut()
//! {
//!     node.entry("timeout").or_insert(5.into());
//! }
//! ```

use serde_json::Value;

use ephys_rs::core::config_patching::{SavedPart, SavedVersions};

pub fn patch(saved: &SavedVersions, what: SavedPart, value: &mut Value) {
    let _ = (saved, what, value);
}
