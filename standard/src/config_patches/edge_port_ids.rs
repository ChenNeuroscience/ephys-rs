//! Edges' ends by port id, from 0.284.0: each end of an edge named its port by its index among its node's ports of its
//! role (`{"node": 1, "role": "DataStreamOut", "port_idx": 2}`). It names it by the port's id now, its field's name in
//! the node's `ports!` (`{"node": 1, "role": "DataStreamOut", "port": "auxiliary"}`), so the standard node types'
//! ends are renamed here, by their ports as they were then (`PORTS`). An end with an index its node type didn't have
//! couldn't have been connected, so its edge goes. A build's own node types' ends are its patches'.

use ephys_rs::config_patching::IncomingConfig;
use serde_json::{Value, json};

/// Each standard node type's ports of each role, by index, as they were.
/// A role's ports' ids, in order (`("DataStreamOut", &["video", "stats"])`).
type RolePorts = (&'static str, &'static [&'static str]);

const PORTS: &[(&str, &[RolePorts])] = &[
    ("AcquisitionBoard", &[("DataStreamOut", &["headstages", "digital", "auxiliary", "analogue"])]),
    ("Camera", &[("DataStreamOut", &["video", "stats"]), ("ValOutCmdIn", &["camera"])]),
    (
        "Reference",
        &[
            ("DataStreamIn", &["continuous"]),
            ("DataStreamOut", &["referenced", "reference"]),
            ("ValOutCmdIn", &["reference"]),
        ],
    ),
    (
        "ChannelSelector",
        &[("DataStreamIn", &["continuous"]), ("DataStreamOut", &["selected"]), ("ValOutCmdIn", &["selection"])],
    ),
    ("BandPassFilter", &[("DataStreamIn", &["continuous"]), ("DataStreamOut", &["filtered"])]),
    ("Downsample", &[("DataStreamIn", &["continuous"]), ("DataStreamOut", &["downsampled"])]),
    ("Spectrum", &[("DataStreamIn", &["continuous"]), ("DataStreamOut", &["spectra"])]),
    (
        "ContinuousGain",
        &[("DataStreamIn", &["continuous"]), ("DataStreamOut", &["scaled"]), ("ValOutCmdIn", &["gains"])],
    ),
    (
        "SpikeDetector",
        &[("DataStreamIn", &["continuous"]), ("DataStreamOut", &["spikes"]), ("ValOutCmdIn", &["thresholds"])],
    ),
    ("SpikeGain", &[("DataStreamIn", &["spikes"]), ("DataStreamOut", &["spikes"]), ("ValOutCmdIn", &["gains"])]),
    (
        "PositionDetector",
        &[("DataStreamIn", &["video"]), ("DataStreamOut", &["positions", "pos_ttl"]), ("ValOutCmdIn", &["detection"])],
    ),
    (
        "AlarmProcessor",
        &[
            ("DataStreamIn", &["spikes", "lfp", "positions", "ttl"]),
            ("DataStreamOut", &["alarms"]),
            ("ValOutCmdIn", &["alarms"]),
        ],
    ),
    (
        "LfpViewer",
        &[
            ("DataStreamIn", &["continuous", "spectra", "reference"]),
            ("ValInCmdOut", &["gains", "selection", "reference"]),
        ],
    ),
    ("SpikeViewer", &[("DataStreamIn", &["spikes"]), ("ValInCmdOut", &["thresholds", "gains"])]),
    ("TtlViewer", &[("DataStreamIn", &["events"])]),
    ("LogViewer", &[("DataStreamIn", &["log"])]),
    ("VideoViewer", &[("DataStreamIn", &["video"])]),
    ("TrackerView", &[("DataStreamIn", &["video", "positions", "stats"]), ("ValInCmdOut", &["camera", "detection"])]),
    ("AlarmViewer", &[("DataStreamIn", &["alarms"]), ("ValInCmdOut", &["alarms"])]),
    (
        "WriteLtx",
        &[
            ("DataStreamIn", &["spikes", "eeg", "positions", "events", "pos_ttl", "log"]),
            ("ValInCmdOut", &["after_recording"]),
            ("ValIn", &["detection", "gains", "eeg_selection", "eeg_gains"]),
        ],
    ),
    ("WriteRaw", &[("DataStreamIn", &["continuous", "events"]), ("ValInCmdOut", &["after_recording"])]),
    ("RunCommand", &[("ValOutCmdIn", &["files"]), ("ValInCmdOut", &["after_running"])]),
    ("SyncPositions", &[("ValOutCmdIn", &["files"]), ("ValInCmdOut", &["after_syncing"])]),
    ("TtlRecordingControl", &[("DataStreamIn", &["events"])]),
];

pub fn patch(config: &mut IncomingConfig) {
    if !config.written_before("std", "0.284.0") {
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
    use serde_json::{Value, json};

    use ephys_rs::config_patching::{IncomingConfig, Versions};

    fn end(node: u64, role: &str, port_idx: u64) -> Value {
        json!({ "node": node, "role": role, "port_idx": port_idx })
    }

    fn project() -> Value {
        json!({ "graph": {
            "nodes": {
                "1": { "node": { "type": "AcquisitionBoard" } },
                "2": { "node": { "type": "WriteLtx" } },
                "3": { "node": { "type": "SpikeGain" } },
                "4": { "node": { "type": "SomeoneElsesNode" } },
            },
            "edges": [
                { "from": end(1, "DataStreamOut", 0), "to": end(2, "DataStreamIn", 1) },
                { "from": end(2, "ValIn", 1), "to": end(3, "ValOutCmdIn", 0) },
                { "from": end(1, "DataStreamOut", 2), "to": end(4, "DataStreamIn", 0) },
                { "from": end(1, "DataStreamOut", 9), "to": end(2, "DataStreamIn", 0) },
            ],
            "disabled_edges": [{ "from": end(1, "DataStreamOut", 0), "to": end(2, "DataStreamIn", 1) }],
        } })
    }

    /// Each standard node's end is named by its port's id; another set's node's is left for its patches; an end its
    /// node type never had goes, with its edge.
    #[test]
    fn standard_nodes_ends_are_named_by_their_ports_ids() {
        let mut config = IncomingConfig { versions: Versions::new(), json: project() };
        super::patch(&mut config);
        let named = |node: u64, role: &str, port: &str| json!({ "node": node, "role": role, "port": port });
        let expected = json!([
            { "from": named(1, "DataStreamOut", "headstages"), "to": named(2, "DataStreamIn", "eeg") },
            { "from": named(2, "ValIn", "gains"), "to": named(3, "ValOutCmdIn", "gains") },
            { "from": named(1, "DataStreamOut", "auxiliary"), "to": end(4, "DataStreamIn", 0) },
        ]);
        assert_eq!(config.json["graph"]["edges"], expected);
        assert_eq!(config.json["graph"]["disabled_edges"], json!([expected[0]]));
    }

    /// Written since, as it is.
    #[test]
    fn written_since_is_left_as_it_is() {
        let versions = Versions::from([("std".to_string(), "0.284.0".to_string())]);
        let mut config = IncomingConfig { versions, json: project() };
        super::patch(&mut config);
        assert_eq!(config.json, project());
    }
}
