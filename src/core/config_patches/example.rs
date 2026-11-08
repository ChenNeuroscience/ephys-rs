//! An example, of the shape a patch takes, to copy: it doesn't do anything. A patch is a script: it's given the versions
//! that saved something, and that, as a project's JSON (see `crate::core::config_patching`), and changes what was
//! saved before into what's saved now, if it needs to. Moving the layout under a key of its own, from the version after
//! 0.158.0, say:
//!
//! ```ignore
//! if saved_before(saved, "core", "0.159.0")
//!     && let Some(layout) = project.as_object_mut().and_then(|p| p.remove("layout"))
//! {
//!     project["windows"] = serde_json::json!({ "layout": layout });
//! }
//! ```

use serde_json::Value;

use crate::core::config_patching::SavedVersions;

pub fn patch(saved: &SavedVersions, project: &mut Value) {
    let _ = (saved, project);
}
