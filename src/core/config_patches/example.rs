//! Level 0 to 1: nothing changed. An example, of the shape a patch takes, to copy: it matches what it's about, and
//! changes that as JSON, from what was saved before to what's saved now. Moving the layout under a key of its own, say:
//!
//! ```ignore
//! if what == SavedPart::Project
//!     && let Some(project) = value.as_object_mut()
//!     && let Some(layout) = project.remove("layout")
//! {
//!     project.insert("windows".into(), serde_json::json!({ "layout": layout }));
//! }
//! ```

use serde_json::Value;

use crate::core::config_patching::SavedPart;

pub fn patch(what: SavedPart, value: &mut Value) {
    let _ = (what, value);
}
