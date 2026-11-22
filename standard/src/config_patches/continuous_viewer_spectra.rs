//! The Continuous Data Viewer without spectra, from 0.294.0. Before that version, the viewer had a `spectra` input,
//! for a Spectrum node's output, and a `spectrum` setting saying whether to show each channel's spectrum beside its
//! trace. The Phase Viewer now shows what the spectra were for, so the viewer has neither. This patch removes the
//! setting, and the edges into the viewer's `spectra` input (which loading would otherwise drop with a warning).

use ephys_rs::config_patching::{IncomingConfig, Patch};
use serde_json::Value;

pub const PATCH: Patch = Patch::new("0.294.0", patch);

fn patch(config: &mut IncomingConfig) {
    let Some(graph) = config.json.get_mut("graph") else { return };
    let mut viewers: Vec<u64> = Vec::new();
    for (id, entry) in graph.get_mut("nodes").and_then(Value::as_object_mut).into_iter().flatten() {
        let node = &mut entry["node"];
        if node["type"] != "ContinuousViewer" {
            continue;
        }
        if let Some(settings) = node.get_mut("settings").and_then(Value::as_object_mut) {
            settings.remove("spectrum");
        }
        viewers.extend(id.parse::<u64>().ok());
    }
    let into_spectra = |end: &Value| {
        end["node"].as_u64().is_some_and(|node| viewers.contains(&node))
            && end["role"] == "DataStreamIn"
            && end["port"] == "spectra"
    };
    // Disabled edges are saved in a list of their own, so both lists are patched.
    for list in ["edges", "disabled_edges"] {
        if let Some(edges) = graph.get_mut(list).and_then(Value::as_array_mut) {
            edges.retain(|edge| !into_spectra(&edge["to"]));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use ephys_rs::config_patching::{IncomingConfig, Versions};

    fn end(node: u64, role: &str, port: &str) -> Value {
        json!({ "node": node, "role": role, "port": port })
    }

    fn project() -> Value {
        json!({ "graph": {
            "nodes": {
                "1": { "node": { "type": "Spectrum", "settings": {} } },
                "2": { "node": { "type": "ContinuousViewer", "settings": { "span": 2.0, "spectrum": true } } },
                "3": { "node": { "type": "BandPassFilter", "settings": {} } },
            },
            "edges": [
                { "from": end(1, "DataStreamOut", "spectra"), "to": end(2, "DataStreamIn", "spectra") },
                { "from": end(3, "DataStreamOut", "filtered"), "to": end(2, "DataStreamIn", "continuous") },
                { "from": end(3, "DataStreamOut", "filtered"), "to": end(1, "DataStreamIn", "continuous") },
            ],
            "disabled_edges": [{ "from": end(1, "DataStreamOut", "spectra"), "to": end(2, "DataStreamIn", "spectra") }],
        } })
    }

    /// The viewer loses its spectrum setting and the edge into its spectra input; the Spectrum node and every other
    /// edge stay.
    #[test]
    fn the_viewer_loses_its_spectra() {
        let mut config = IncomingConfig { versions: Versions::new(), json: project() };
        super::PATCH.apply_to("std", &mut config);
        let graph = &config.json["graph"];
        assert_eq!(graph["nodes"]["2"]["node"]["settings"], json!({ "span": 2.0 }));
        assert_eq!(graph["nodes"]["1"]["node"]["type"], "Spectrum");
        assert_eq!(graph["edges"], json!([project()["graph"]["edges"][1], project()["graph"]["edges"][2]]));
        assert_eq!(graph["disabled_edges"], json!([]));
    }

    /// A config written since 0.294.0 is left as it is.
    #[test]
    fn written_since_is_left_as_it_is() {
        let versions = Versions::from([("std".to_string(), "0.294.0".to_string())]);
        let mut config = IncomingConfig { versions, json: project() };
        super::PATCH.apply_to("std", &mut config);
        assert_eq!(config.json, project());
    }
}
