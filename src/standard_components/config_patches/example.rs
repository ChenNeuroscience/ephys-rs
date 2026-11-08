//! Level 0 to 1: nothing changed. An example, of the shape a patch takes, to copy: it matches what it's about, and
//! changes that as JSON, from what was saved before to what's saved now. Renaming a setting of the Spike Detector, say:
//!
//! ```ignore
//! if what == SavedPart::Node("SpikeDetector")
//!     && let Some(node) = value.as_object_mut()
//!     && let Some(old) = node.remove("threshold")
//! {
//!     node.insert("thresholds".into(), old);
//! }
//! ```

use serde_json::Value;

use crate::core::config_patching::SavedPart;

pub fn patch(what: SavedPart, value: &mut Value) {
    let _ = (what, value);
}
