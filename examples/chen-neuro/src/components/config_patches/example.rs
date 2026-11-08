//! An example, of the shape a patch takes, to copy: it doesn't do anything. A patch is a script: it's given config coming in
//! (`IncomingConfig`): the versions that wrote it, and it, as a project's JSON (see `ephys_rs::core::config_patching`), and changes what was
//! written before into what's written now, if it needs to. Giving the VR Computer a setting it didn't have, from the version
//! after 0.1007.0, say:
//!
//! ```ignore
//! if config.written_before("chen-neuro", "0.1008.0") {
//!     for node in config.nodes().filter(|node| node["type"] == "VrComputer") {
//!         node["settings"]["timeout"] = 5.into();
//!     }
//! }
//! ```

use ephys_rs::core::config_patching::IncomingConfig;

pub fn patch(config: &mut IncomingConfig) {
    let _ = config;
}
