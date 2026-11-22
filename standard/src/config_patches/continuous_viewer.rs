//! The Continuous Data Viewer, from 0.288.0. This node type was the LFP Viewer (`"type": "LfpViewer"`), but the viewer
//! shows any continuous data, band-passed or not, so it was renamed to the Continuous Data Viewer
//! (`"type": "ContinuousViewer"`). This patch renames the type of each saved node. A node's label is renamed too if
//! it's still the default that the graph gave it ("LFP Viewer", or "LFP Viewer 2" and so on); a label someone chose is
//! kept.

use ephys_rs::config_patching::{IncomingConfig, Patch};
use serde_json::Value;

const OLD_NAME: &str = "LFP Viewer";
const NEW_NAME: &str = "Continuous Data Viewer";

pub const PATCH: Patch = Patch::new("0.288.0", patch);

fn patch(config: &mut IncomingConfig) {
    let entries = config.json.pointer_mut("/graph/nodes").and_then(Value::as_object_mut);
    for entry in entries.into_iter().flat_map(|nodes| nodes.values_mut()) {
        if entry.pointer("/node/type").is_none_or(|kind| kind != "LfpViewer") {
            continue;
        }
        entry["node"]["type"] = "ContinuousViewer".into();
        let Some(label) = entry.get("label").and_then(Value::as_str) else { continue };
        if let Some(suffix) = default_label_suffix(label) {
            entry["label"] = format!("{NEW_NAME}{suffix}").into();
        }
    }
}

/// The part of `label` after the old name ("", or " 2" and so on), if `label` is a default label the graph gave.
fn default_label_suffix(label: &str) -> Option<&str> {
    let suffix = label.strip_prefix(OLD_NAME)?;
    let number = suffix.strip_prefix(' ');
    let is_default =
        suffix.is_empty() || number.is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    is_default.then_some(suffix)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use ephys_rs::config_patching::{IncomingConfig, Versions};

    fn project() -> Value {
        json!({ "graph": { "nodes": {
            "1": { "label": "LFP Viewer", "node": { "type": "LfpViewer", "span": 2.0 } },
            "2": { "label": "LFP Viewer 2", "node": { "type": "LfpViewer" } },
            "3": { "label": "Theta", "node": { "type": "LfpViewer" } },
            "4": { "label": "LFP Viewer", "node": { "type": "SpikeViewer" } },
        } } })
    }

    /// Each LFP Viewer becomes a Continuous Data Viewer, with its settings kept. Default labels are renamed, but a
    /// label someone chose isn't, and nor is another node type's label.
    #[test]
    fn lfp_viewers_become_continuous_data_viewers() {
        let mut config = IncomingConfig { versions: Versions::new(), json: project() };
        super::PATCH.apply_to("std", &mut config);
        let expected = json!({ "graph": { "nodes": {
            "1": { "label": "Continuous Data Viewer", "node": { "type": "ContinuousViewer", "span": 2.0 } },
            "2": { "label": "Continuous Data Viewer 2", "node": { "type": "ContinuousViewer" } },
            "3": { "label": "Theta", "node": { "type": "ContinuousViewer" } },
            "4": { "label": "LFP Viewer", "node": { "type": "SpikeViewer" } },
        } } });
        assert_eq!(config.json, expected);
    }

    /// A config written since the rename is left as it is.
    #[test]
    fn written_since_is_left_as_it_is() {
        let versions = Versions::from([("std".to_string(), "0.288.0".to_string())]);
        let mut config = IncomingConfig { versions, json: project() };
        super::PATCH.apply_to("std", &mut config);
        assert_eq!(config.json, project());
    }

    #[test]
    fn only_default_labels_count() {
        assert_eq!(super::default_label_suffix("LFP Viewer"), Some(""));
        assert_eq!(super::default_label_suffix("LFP Viewer 12"), Some(" 12"));
        assert_eq!(super::default_label_suffix("LFP Viewer two"), None);
        assert_eq!(super::default_label_suffix("LFP Viewer "), None);
        assert_eq!(super::default_label_suffix("LFP Viewers"), None);
    }
}
