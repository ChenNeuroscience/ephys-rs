//! The Spike Gain reading the Spike Detector's thresholds, from 0.299.0. Before that version, the thresholds came with
//! the spikes (on each `SpikesBuffer`), and the gains' auto detect went by them there. The Spike Gain now reads them
//! through a `thresholds` port of its own (a `ValIn`), connected to the detector's `thresholds`. So this patch connects
//! that port, for each Spike Gain whose spikes come straight from a Spike Detector (in an edge enabled or not).

use ephys_rs::config_patching::{IncomingConfig, Patch};
use serde_json::{Value, json};

pub const PATCH: Patch = Patch::new("0.299.0", patch);

fn patch(config: &mut IncomingConfig) {
    let Some(graph) = config.json.get_mut("graph") else { return };
    let of_type = |node_type: &str| -> Vec<u64> {
        let nodes = graph.get("nodes").and_then(Value::as_object).into_iter().flatten();
        nodes.filter(|(_, entry)| entry["node"]["type"] == node_type).filter_map(|(id, _)| id.parse().ok()).collect()
    };
    let (detectors, gains) = (of_type("SpikeDetector"), of_type("SpikeGain"));
    let is = |end: &Value, nodes: &[u64], role: &str| {
        end["node"].as_u64().is_some_and(|node| nodes.contains(&node)) && end["role"] == role && end["port"] == "spikes"
    };
    // Each Spike Gain fed by a detector, and the detector: a gain has one spikes input, so one each.
    let mut links: Vec<(u64, u64)> = Vec::new();
    for list in ["edges", "disabled_edges"] {
        for edge in graph.get(list).and_then(Value::as_array).into_iter().flatten() {
            let (from, to) = (&edge["from"], &edge["to"]);
            if is(from, &detectors, "DataStreamOut") && is(to, &gains, "DataStreamIn") {
                let (Some(detector), Some(gain)) = (from["node"].as_u64(), to["node"].as_u64()) else { continue };
                if !links.iter().any(|&(_, g)| g == gain) {
                    links.push((gain, detector));
                }
            }
        }
    }
    if links.is_empty() {
        return;
    }
    let Some(edges) = graph.as_object_mut().map(|g| g.entry("edges").or_insert_with(|| json!([]))) else { return };
    let Some(edges) = edges.as_array_mut() else { return };
    for (gain, detector) in links {
        edges.push(json!({
            "from": { "node": gain, "role": "ValIn", "port": "thresholds" },
            "to": { "node": detector, "role": "ValOutCmdIn", "port": "thresholds" },
        }));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use ephys_rs::config_patching::{IncomingConfig, Versions};

    fn end(node: u64, role: &str, port: &str) -> Value {
        json!({ "node": node, "role": role, "port": port })
    }

    fn thresholds(gain: u64, detector: u64) -> Value {
        json!({ "from": end(gain, "ValIn", "thresholds"), "to": end(detector, "ValOutCmdIn", "thresholds") })
    }

    fn project() -> Value {
        json!({ "graph": {
            "nodes": {
                "1": { "node": { "type": "SpikeDetector", "settings": {} } },
                "2": { "node": { "type": "SpikeGain", "settings": {} } },
                "3": { "node": { "type": "SpikeDetector", "settings": {} } },
                "4": { "node": { "type": "SpikeGain", "settings": {} } },
                "5": { "node": { "type": "SpikeGain", "settings": {} } },
                "6": { "node": { "type": "SpikeViewer", "settings": {} } },
            },
            "edges": [
                { "from": end(1, "DataStreamOut", "spikes"), "to": end(2, "DataStreamIn", "spikes") },
                { "from": end(2, "DataStreamOut", "spikes"), "to": end(5, "DataStreamIn", "spikes") },
                { "from": end(2, "DataStreamOut", "spikes"), "to": end(6, "DataStreamIn", "spikes") },
            ],
            "disabled_edges": [{ "from": end(3, "DataStreamOut", "spikes"), "to": end(4, "DataStreamIn", "spikes") }],
        } })
    }

    /// Each Spike Gain fed straight by a detector, by an enabled edge or a disabled one, reads its thresholds; a gain
    /// fed by another gain doesn't.
    #[test]
    fn gains_fed_by_a_detector_read_its_thresholds() {
        let mut config = IncomingConfig { versions: Versions::new(), json: project() };
        super::PATCH.apply_to("std", &mut config);
        let mut expected = project()["graph"]["edges"].as_array().unwrap().clone();
        expected.extend([thresholds(2, 1), thresholds(4, 3)]);
        assert_eq!(config.json["graph"]["edges"], json!(expected));
        assert_eq!(config.json["graph"]["disabled_edges"], project()["graph"]["disabled_edges"]);
    }

    /// A config written since 0.299.0 is left as it is.
    #[test]
    fn written_since_is_left_as_it_is() {
        let versions = Versions::from([("std".to_string(), "0.299.0".to_string())]);
        let mut config = IncomingConfig { versions, json: project() };
        super::PATCH.apply_to("std", &mut config);
        assert_eq!(config.json, project());
    }
}
