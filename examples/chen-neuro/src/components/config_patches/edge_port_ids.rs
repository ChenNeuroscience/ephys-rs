//! VR Computer's edges' ends by port id, from 0.1134.0, as the standard components' patch `edge_port_ids` does for
//! theirs: each end named its port by its index among its node's ports of its role, and names it by the port's id now
//! (its field's name in the node's `ports!`). An end with an index its node type didn't have couldn't have been
//! connected, so its edge goes.

use ephys_rs::config_patching::IncomingConfig;
use serde_json::{Value, json};

/// This build's own node types' ports of each role, by index, as they were.
/// A role's ports' ids, in order (`("DataStreamOut", &["video", "stats"])`).
type RolePorts = (&'static str, &'static [&'static str]);

const PORTS: &[(&str, &[RolePorts])] =
    &[("VrComputer", &[("DataStreamOut", &["positions", "pos_ttl", "log"]), ("ValOutCmdIn", &["settings"])])];

pub fn patch(config: &mut IncomingConfig) {
    if !config.written_before("chen-neuro", "0.1134.0") {
        return;
    }
    let Some(graph) = config.json.get_mut("graph") else { return };
    let types: Vec<(u64, String)> = (graph.get("nodes").and_then(Value::as_object).into_iter().flatten())
        .filter_map(|(id, entry)| Some((id.parse().ok()?, entry.pointer("/node/type")?.as_str()?.to_string())))
        .collect();
    // The id of the port at `end`, if it's a standard node's: `Some(None)` if its node type had no such port.
    let id = |end: &Value| -> Option<Option<&'static str>> {
        let node = end["node"].as_u64()?;
        let node_type = &types.iter().find(|(id, _)| *id == node)?.1;
        let (_, roles) = PORTS.iter().find(|(name, _)| name == node_type)?;
        let ports = roles.iter().find(|(role, _)| end["role"] == *role).map_or(&[][..], |(_, ports)| ports);
        Some(end["port_idx"].as_u64().and_then(|index| ports.get(index as usize).copied()))
    };
    // A disabled edge is listed among the edges too.
    for list in ["edges", "disabled_edges"] {
        let Some(edges) = graph.get_mut(list).and_then(Value::as_array_mut) else { continue };
        edges.retain_mut(|edge| {
            for end in ["from", "to"] {
                match id(&edge[end]) {
                    Some(Some(port)) => {
                        let (node, role) = (edge[end]["node"].clone(), edge[end]["role"].clone());
                        edge[end] = json!({ "node": node, "role": role, "port": port });
                    }
                    Some(None) => return false,
                    None => {}
                }
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use ephys_rs::config_patching::{IncomingConfig, Versions};

    /// VR Computer's ends are named by their ports' ids; a standard node's are left for its patches.
    #[test]
    fn vr_computers_ends_are_named_by_their_ports_ids() {
        let json = json!({ "graph": {
            "nodes": { "1": { "node": { "type": "VrComputer" } }, "2": { "node": { "type": "WriteLtx" } } },
            "edges": [{
                "from": { "node": 1, "role": "DataStreamOut", "port_idx": 1 },
                "to": { "node": 2, "role": "DataStreamIn", "port_idx": 4 },
            }],
        } });
        let mut config = IncomingConfig { versions: Versions::new(), json };
        super::patch(&mut config);
        let expected = json!([{
            "from": { "node": 1, "role": "DataStreamOut", "port": "pos_ttl" },
            "to": { "node": 2, "role": "DataStreamIn", "port_idx": 4 },
        }]);
        assert_eq!(config.json["graph"]["edges"], expected);
    }
}
