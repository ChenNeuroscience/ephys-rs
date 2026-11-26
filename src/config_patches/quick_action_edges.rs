//! Quick actions' edges, from 0.315.0: each edge had a mode (`"mode"`: `ensure`, the default, connected if the end
//! that takes just the one edge was free; `replace`, connected whatever; `remove`, disconnected; `ignore`, left alone).
//! An edge now says what's done in each case (see `crate::quick_action::EdgeRule`): `enabled` (or disabled) if
//! connected already, `connect` if that end's free (true unless it says not), and `replace` if that end's connected
//! to something else. Nothing's disconnected any more, so a removed edge is now disabled instead.
//! Projects' edges have no mode, so this changes only quick actions' files.

use serde_json::Value;

use crate::config_patching::{IncomingConfig, Patch};

pub const PATCH: Patch = Patch::new("0.315.0", patch);

fn patch(config: &mut IncomingConfig) {
    let Some(edges) = config.json.pointer_mut("/graph/edges").and_then(Value::as_array_mut) else { return };
    for edge in edges {
        let Some(edge) = edge.as_object_mut() else { continue };
        let Some(mode) = edge.remove("mode") else { continue };
        match mode.as_str() {
            Some("replace") => {
                edge.insert("replace".into(), true.into());
            }
            Some("remove") => {
                // Disconnecting is gone: disabling leaves the edge doing nothing, as removing it did.
                edge.insert("enabled".into(), false.into());
                edge.insert("connect".into(), false.into());
            }
            Some("ignore") => {
                edge.insert("connect".into(), false.into());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::config_patching::{IncomingConfig, Versions};

    /// Each mode becomes what it did in each case; an edge without one (ensured) is left as it is.
    #[test]
    fn each_mode_becomes_what_it_did() {
        let edge = |mode: Option<&str>| {
            let mut edge = json!({ "from": {}, "to": {} });
            if let Some(mode) = mode {
                edge["mode"] = mode.into();
            }
            edge
        };
        let modes = [None, Some("ensure"), Some("replace"), Some("remove"), Some("ignore")];
        let mut config = IncomingConfig {
            json: json!({ "graph": { "nodes": {}, "edges": modes.map(edge) } }),
            versions: Versions::new(),
        };
        super::patch(&mut config);
        let ends = json!({ "from": {}, "to": {} });
        let with = |extra: serde_json::Value| {
            let mut edge = ends.clone();
            edge.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            edge
        };
        assert_eq!(
            config.json["graph"]["edges"],
            json!([
                ends,
                ends,
                with(json!({ "replace": true })),
                with(json!({ "enabled": false, "connect": false })),
                with(json!({ "connect": false })),
            ])
        );
    }
}
