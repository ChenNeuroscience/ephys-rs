//! Edge dimensions' names, from 0.276.0: an edge saved its dimension as `Data` or `Settings` (or, briefly, `Values`).
//! They're now `DataStream` and `ValCmd`, after the roles of the ports they join (see `crate::engine::Dimension`).

use serde_json::Value;

use crate::config_patching::IncomingConfig;

pub fn patch(config: &mut IncomingConfig) {
    if !config.written_before("core", "0.276.0") {
        return;
    }
    let Some(graph) = config.json.get_mut("graph") else { return };
    // A disabled edge is listed among the edges too.
    for list in ["edges", "disabled_edges"] {
        let Some(edges) = graph.get_mut(list).and_then(Value::as_array_mut) else { continue };
        for edge in edges {
            let renamed = match edge.get("dimension").and_then(Value::as_str) {
                Some("Data") => "DataStream",
                Some("Settings" | "Values") => "ValCmd",
                _ => continue,
            };
            edge["dimension"] = renamed.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::config_patching::{IncomingConfig, Versions};

    fn edge(dimension: &str) -> Value {
        json!({ "dimension": dimension, "from": { "node": 1, "port": 0 }, "to": { "node": 2, "port": 0 } })
    }

    fn patched(versions: Versions) -> Vec<Value> {
        let edges = vec![edge("Data"), edge("Settings"), edge("Values")];
        let graph = json!({ "nodes": {}, "edges": edges, "disabled_edges": [edge("Settings")] });
        let mut config = IncomingConfig { versions, json: json!({ "graph": graph }) };
        super::patch(&mut config);
        let graph = &config.json["graph"];
        let dimensions = graph["edges"].as_array().unwrap().iter().chain(graph["disabled_edges"].as_array().unwrap());
        dimensions.map(|edge| edge["dimension"].clone()).collect()
    }

    /// Each takes its new name, disabled or not, and the edges load.
    #[test]
    fn edges_dimensions_take_their_new_names() {
        assert_eq!(patched(Versions::new()), [json!("DataStream"), json!("ValCmd"), json!("ValCmd"), json!("ValCmd")]);
        let edge: crate::engine::Edge = serde_json::from_value(edge("ValCmd")).unwrap();
        assert_eq!(edge.dimension, crate::engine::Dimension::ValCmd);
    }

    /// Written since, as it is.
    #[test]
    fn edges_written_since_are_left_as_they_are() {
        let versions = Versions::from([("core".to_string(), "0.276.0".to_string())]);
        assert_eq!(patched(versions), [json!("Data"), json!("Settings"), json!("Values"), json!("Settings")]);
    }
}
