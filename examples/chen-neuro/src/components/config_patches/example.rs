//! Level 0 to 1: nothing changed. An example, of the shape a patch takes, to copy: it matches what it's about, and
//! changes that as JSON, from what was saved before to what's saved now. Giving the VR Computer a setting it didn't have, say:
//!
//! ```ignore
//! if what == SavedPart::Node("VrComputer")
//!     && let Some(node) = value.as_object_mut()
//! {
//!     node.entry("timeout").or_insert(5.into());
//! }
//! ```

use serde_json::Value;

use ephys_rs::core::config_patching::SavedPart;

pub fn patch(what: SavedPart, value: &mut Value) {
    let _ = (what, value);
}
