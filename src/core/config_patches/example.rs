//! An example, of the shape a patch takes, to copy: it doesn't do anything. A patch is a script: it's given config coming in
//! (`IncomingConfig`): the versions that wrote it, and it, as a project's JSON (see `crate::core::config_patching`), and changes what was
//! written before into what's written now, if it needs to. Moving the layout under a key of its own, from the version after
//! 0.158.0, say:
//!
//! ```ignore
//! if config.written_before("core", "0.159.0")
//!     && let Some(layout) = config.json.as_object_mut().and_then(|p| p.remove("layout"))
//! {
//!     config.json["windows"] = serde_json::json!({ "layout": layout });
//! }
//! ```

use crate::core::config_patching::IncomingConfig;

pub fn patch(config: &mut IncomingConfig) {
    let _ = config;
}
