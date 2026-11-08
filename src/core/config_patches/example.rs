//! An example, of the shape a patch takes, to copy: it doesn't do anything. A patch decides for itself whether to, from
//! the versions that saved what it's given (see `crate::core::config_patching`), and what that is, and changes it as
//! JSON, from what was saved before to what's saved now. Moving the layout under a key of its own, from the version after
//! 0.157.0, say:
//!
//! ```ignore
//! if what == SavedPart::Project
//!     && saved_before(saved, "core", "0.158.0")
//!     && let Some(project) = value.as_object_mut()
//!     && let Some(layout) = project.remove("layout")
//! {
//!     project.insert("windows".into(), serde_json::json!({ "layout": layout }));
//! }
//! ```

use serde_json::Value;

use crate::core::config_patching::{SavedPart, SavedVersions};

pub fn patch(saved: &SavedVersions, what: SavedPart, value: &mut Value) {
    let _ = (saved, what, value);
}
