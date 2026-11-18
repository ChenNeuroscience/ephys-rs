//! Edges' ends, from 0.270.0: an edge saved its dimension (`Data`, the default, or `Settings`) and each end as a node
//! and a port index (`{"node": 1, "port": 0}`), the ends' roles following from the dimension. Each end is now the
//! port's whole address, role and all (`{"node": 1, "role": "DataStreamOut", "port_idx": 0}`: see
//! `crate::engine::PortAddress`), and there's no dimension. A `Settings` edge ran from a control (now a `ValInCmdOut`,
//! or a `ValIn`, which a component's patch sees to, as only it knows its nodes) to a setting (now a `ValOutCmdIn`).

use serde_json::{Value, json};

use crate::config_patching::IncomingConfig;

pub fn patch(config: &mut IncomingConfig) {
    if !config.written_before("core", "0.270.0") {
        return;
    }
    let Some(graph) = config.json.get_mut("graph") else { return };
    // A disabled edge is listed among the edges too.
    for list in ["edges", "disabled_edges"] {
        let Some(edges) = graph.get_mut(list).and_then(Value::as_array_mut) else { continue };
        for edge in edges {
            let (from, to) = match edge.get("dimension").and_then(Value::as_str) {
                Some("Settings") => ("ValInCmdOut", "ValOutCmdIn"),
                _ => ("DataStreamOut", "DataStreamIn"),
            };
            let end = |end: &Value, role: &str| json!({ "node": end["node"], "role": role, "port_idx": end["port"] });
            *edge = json!({ "from": end(&edge["from"], from), "to": end(&edge["to"], to) });
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::config_patching::{IncomingConfig, Versions};

    fn project() -> Value {
        let edge = |dimension: Option<&str>| {
            let mut edge = json!({ "from": { "node": 1, "port": 2 }, "to": { "node": 3, "port": 4 } });
            if let Some(dimension) = dimension {
                edge["dimension"] = dimension.into();
            }
            edge
        };
        json!({ "graph": {
            "nodes": {},
            "edges": [edge(Some("Data")), edge(None), edge(Some("Settings"))],
            "disabled_edges": [edge(Some("Settings"))],
        } })
    }

    /// Each end becomes the port's address, its role from the edge's dimension; disabled edges too.
    #[test]
    fn edges_ends_become_ports_addresses() {
        let mut config = IncomingConfig { versions: Versions::new(), json: project() };
        super::patch(&mut config);
        let addressed = |from: &str, to: &str| {
            json!({
                "from": { "node": 1, "role": from, "port_idx": 2 },
                "to": { "node": 3, "role": to, "port_idx": 4 },
            })
        };
        let (data, value) = (addressed("DataStreamOut", "DataStreamIn"), addressed("ValInCmdOut", "ValOutCmdIn"));
        let graph = &config.json["graph"];
        assert_eq!(graph["edges"], json!([data, data, value]));
        assert_eq!(graph["disabled_edges"], json!([value]));
        let edge: crate::engine::Edge = serde_json::from_value(value).unwrap();
        assert!(!edge.carries_data_stream(), "and they load");
    }

    /// Written since, as it is.
    #[test]
    fn edges_written_since_are_left_as_they_are() {
        let versions = Versions::from([("core".to_string(), "0.270.0".to_string())]);
        let mut config = IncomingConfig { versions, json: project() };
        super::patch(&mut config);
        assert_eq!(config.json, project());
    }
}
