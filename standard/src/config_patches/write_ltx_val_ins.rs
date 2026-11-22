//! Write LTX's ports reading values, from 0.270.0: its controls (detection, gains, eeg selection, eeg gains, then after
//! recording) were numbered together, so an edge from its control 4 (after recording) said so. Its first four only read
//! what they're connected to, and the graph now numbers such ports (`ValIn`) apart from those that also send commands
//! (`ValInCmdOut`). So, after the core's patch has given each edge's end its role (as a `ValInCmdOut`), the first four
//! are its `ValIn` ports 0 to 3, and after recording is its `ValInCmdOut` port 0. No other standard node had both.

use ephys_rs::config_patching::{IncomingConfig, Patch};
use serde_json::Value;

/// Write LTX's controls that only read: those numbered below this.
const READ_ONLY: u64 = 4;

pub const PATCH: Patch = Patch::new("0.270.0", patch);

fn patch(config: &mut IncomingConfig) {
    let Some(graph) = config.json.get_mut("graph") else { return };
    let nodes = graph.get("nodes").and_then(Value::as_object);
    let writers: Vec<u64> = nodes
        .into_iter()
        .flatten()
        .filter(|(_, entry)| entry.pointer("/node/type").and_then(Value::as_str) == Some("WriteLtx"))
        .filter_map(|(id, _)| id.parse().ok())
        .collect();
    // A disabled edge is listed among the edges too.
    for list in ["edges", "disabled_edges"] {
        let Some(edges) = graph.get_mut(list).and_then(Value::as_array_mut) else { continue };
        for from in edges.iter_mut().map(|edge| &mut edge["from"]) {
            let writes = from["node"].as_u64().is_some_and(|node| writers.contains(&node));
            let (Some(port_idx), true) = (from["port_idx"].as_u64(), writes && from["role"] == "ValInCmdOut") else {
                continue;
            };
            if port_idx < READ_ONLY {
                from["role"] = "ValIn".into();
            } else {
                from["port_idx"] = (port_idx - READ_ONLY).into();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use ephys_rs::config_patching::{IncomingConfig, Versions};

    fn project() -> Value {
        let from = |node: u64, port_idx: u64| {
            json!({
                "from": { "node": node, "role": "ValInCmdOut", "port_idx": port_idx },
                "to": { "node": 9, "role": "ValOutCmdIn", "port_idx": 0 },
            })
        };
        json!({ "graph": {
            "nodes": { "1": { "node": { "type": "WriteLtx" } }, "2": { "node": { "type": "SpikeViewer" } } },
            "edges": [from(1, 1), from(1, 4), from(2, 1)],
            "disabled_edges": [from(1, 3)],
        } })
    }

    fn froms(config: &IncomingConfig, list: &str) -> Vec<(String, u64)> {
        let edges = config.json["graph"][list].as_array().unwrap().iter();
        edges
            .map(|e| (e["from"]["role"].as_str().unwrap().to_string(), e["from"]["port_idx"].as_u64().unwrap()))
            .collect()
    }

    /// Write LTX's first four are `ValIn` ports, and after recording its first `ValInCmdOut`; others' are as they were.
    #[test]
    fn write_ltxs_read_only_controls_are_val_in_ports() {
        let mut config = IncomingConfig { versions: Versions::new(), json: project() };
        super::PATCH.apply_to("std", &mut config);
        let (val_in, val_in_cmd_out) = ("ValIn".to_string(), "ValInCmdOut".to_string());
        assert_eq!(froms(&config, "edges"), [(val_in.clone(), 1), (val_in_cmd_out.clone(), 0), (val_in_cmd_out, 1)]);
        assert_eq!(froms(&config, "disabled_edges"), [(val_in, 3)]);
    }

    /// Written since, as it is.
    #[test]
    fn edges_written_since_are_left_as_they_are() {
        let versions = Versions::from([("std".to_string(), "0.270.0".to_string())]);
        let mut config = IncomingConfig { versions, json: project() };
        super::PATCH.apply_to("std", &mut config);
        assert_eq!(config.json, project());
    }
}
