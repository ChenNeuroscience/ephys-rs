//! The Acquisition Board's outputs, from 0.282.0: its auxiliary and analogue outputs were there only while its `aux` and
//! `adc` settings were on, after its headstages (0) and digital (1) outputs, so with `aux` off, analogue was output 2.
//! The four are always there now, in that order (auxiliary 2, analogue 3), and the settings are gone. So an edge from
//! a board's output 2 where `aux` was off and `adc` on comes from output 3.

use ephys_rs::config_patching::IncomingConfig;
use serde_json::Value;

pub fn patch(config: &mut IncomingConfig) {
    if !config.written_before("std", "0.282.0") {
        return;
    }
    let Some(graph) = config.json.get_mut("graph") else { return };
    let mut moved: Vec<u64> = Vec::new();
    for (id, entry) in graph.get_mut("nodes").and_then(Value::as_object_mut).into_iter().flatten() {
        let node = &mut entry["node"];
        if node["type"] != "AcquisitionBoard" {
            continue;
        }
        let Some(settings) = node.get_mut("settings").and_then(Value::as_object_mut) else { continue };
        let on = |setting: Option<Value>| setting.and_then(|s| s.as_bool()).unwrap_or(false);
        let (aux, adc) = (on(settings.remove("aux")), on(settings.remove("adc")));
        if adc && !aux {
            moved.extend(id.parse::<u64>().ok());
        }
    }
    // A disabled edge is listed among the edges too.
    for list in ["edges", "disabled_edges"] {
        let Some(edges) = graph.get_mut(list).and_then(Value::as_array_mut) else { continue };
        for from in edges.iter_mut().map(|edge| &mut edge["from"]) {
            let board = from["node"].as_u64().is_some_and(|node| moved.contains(&node));
            if board && from["role"] == "DataStreamOut" && from["port_idx"] == 2 {
                from["port_idx"] = 3.into();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use ephys_rs::config_patching::{IncomingConfig, Versions};

    fn board(aux: bool, adc: bool) -> Value {
        json!({ "node": { "type": "AcquisitionBoard", "settings": { "sample_rate": 30000, "aux": aux, "adc": adc } } })
    }

    fn from(node: u64, port_idx: u64) -> Value {
        json!({
            "from": { "node": node, "role": "DataStreamOut", "port_idx": port_idx },
            "to": { "node": 9, "role": "DataStreamIn", "port_idx": 0 },
        })
    }

    fn project() -> Value {
        json!({ "graph": {
            "nodes": { "1": board(false, true), "2": board(true, true), "3": board(false, false) },
            "edges": [from(1, 2), from(1, 1), from(2, 2), from(2, 3), from(3, 0)],
            "disabled_edges": [from(1, 2)],
        } })
    }

    fn ports(config: &IncomingConfig, list: &str) -> Vec<(u64, u64)> {
        let edges = config.json["graph"][list].as_array().unwrap().iter();
        edges.map(|e| (e["from"]["node"].as_u64().unwrap(), e["from"]["port_idx"].as_u64().unwrap())).collect()
    }

    /// Analogue, where auxiliary was off, moves to output 3; the rest stay; the settings go.
    #[test]
    fn analogue_moves_past_auxiliary() {
        let mut config = IncomingConfig { versions: Versions::new(), json: project() };
        super::patch(&mut config);
        assert_eq!(ports(&config, "edges"), [(1, 3), (1, 1), (2, 2), (2, 3), (3, 0)]);
        assert_eq!(ports(&config, "disabled_edges"), [(1, 3)]);
        let settings = &config.json["graph"]["nodes"]["1"]["node"]["settings"];
        assert_eq!(settings, &json!({ "sample_rate": 30000 }));
    }

    /// Settings copied to the clipboard have nodes but no edges: the settings still go.
    #[test]
    fn copied_boards_lose_the_settings() {
        let json = json!({ "graph": { "nodes": { "1": board(true, false) } } });
        let mut config = IncomingConfig { versions: Versions::new(), json };
        super::patch(&mut config);
        assert_eq!(config.json["graph"]["nodes"]["1"]["node"]["settings"], json!({ "sample_rate": 30000 }));
    }

    /// Written since, as it is.
    #[test]
    fn written_since_is_left_as_it_is() {
        let versions = Versions::from([("std".to_string(), "0.282.0".to_string())]);
        let mut config = IncomingConfig { versions, json: project() };
        super::patch(&mut config);
        assert_eq!(config.json, project());
    }
}
