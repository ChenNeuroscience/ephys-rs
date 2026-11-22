//! Edge ends by port id on the VR Computer, from 0.1134.0. The standard components' `edge_port_ids` patch does the
//! same for the standard node types. Before that version, each end of an edge identified its port by the port's number
//! among its node's ports of that role; each end now identifies its port by id, which is the port's field name in the
//! node's `ports!`. An end with a number its node type didn't have could never have been connected, so the patch drops
//! its edge.

use ephys_rs::config_patching::IncomingConfig;
use serde_json::{Value, json};

/// A role and the ids of its ports, in order, such as `("DataStreamOut", &["video", "stats"])`.
type RolePorts = (&'static str, &'static [&'static str]);

/// This build's own node types' ports of each role, in order, as they were at 0.1134.0.
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
    // The id of the port at `end`. Returns `None` if the end's node isn't one of this build's node types (so this patch
    // leaves it alone), and `Some(None)` if the node type had no port with that number.
    let id = |end: &Value| -> Option<Option<&'static str>> {
        let node = end["node"].as_u64()?;
        let node_type = &types.iter().find(|(id, _)| *id == node)?.1;
        let (_, roles) = PORTS.iter().find(|(name, _)| name == node_type)?;
        let ports = roles.iter().find(|(role, _)| end["role"] == *role).map_or(&[][..], |(_, ports)| ports);
        Some(end["port_idx"].as_u64().and_then(|index| ports.get(index as usize).copied()))
    };
    // Disabled edges are saved in a list of their own, so both lists are converted.
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

    /// Ends on the VR Computer are converted to port ids. Ends on standard nodes are left for the standard patches.
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
