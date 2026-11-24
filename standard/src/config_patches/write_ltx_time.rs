//! Write LTX reading the board's time, from 0.303.0. Before that version, Write LTX counted spikes', the EEG's and
//! digital inputs' times each from its own first buffer recorded, so they could be out by however much the first pass
//! had of each. It now counts them all from when recording started by the board's clock, which its `time` input says,
//! from the Acquisition Board's `time` output. So this patch connects that, for each Write LTX whose spikes, EEG or
//! digital inputs come from one Acquisition Board (through whatever's between them, in edges enabled or not), and
//! whose time isn't connected already.

use ephys_rs::config_patching::{IncomingConfig, Patch};
use serde_json::{Value, json};

pub const PATCH: Patch = Patch::new("0.303.0", patch);

/// Write LTX's inputs timed by the board's clock.
const BOARD_TIMED: [&str; 3] = ["spikes", "eeg", "events"];

fn patch(config: &mut IncomingConfig) {
    let Some(graph) = config.json.get_mut("graph") else { return };
    let of_type = |node_type: &str| -> Vec<u64> {
        let nodes = graph.get("nodes").and_then(Value::as_object).into_iter().flatten();
        nodes.filter(|(_, entry)| entry["node"]["type"] == node_type).filter_map(|(id, _)| id.parse().ok()).collect()
    };
    let (boards, writers) = (of_type("AcquisitionBoard"), of_type("WriteLtx"));
    // Data edges, as (from node, to node, to port).
    let mut data: Vec<(u64, u64, String)> = Vec::new();
    for list in ["edges", "disabled_edges"] {
        for edge in graph.get(list).and_then(Value::as_array).into_iter().flatten() {
            let (from, to) = (&edge["from"], &edge["to"]);
            if from["role"] == "DataStreamOut"
                && to["role"] == "DataStreamIn"
                && let (Some(a), Some(b), Some(port)) =
                    (from["node"].as_u64(), to["node"].as_u64(), to["port"].as_str())
            {
                data.push((a, b, port.to_string()));
            }
        }
    }
    let mut links = Vec::new();
    for writer in writers {
        if data.iter().any(|(_, to, port)| *to == writer && port == "time") {
            continue;
        }
        // Everything upstream of its board-timed inputs.
        let mut seen: Vec<u64> = Vec::new();
        let mut stack: Vec<u64> = data
            .iter()
            .filter(|(_, to, port)| *to == writer && BOARD_TIMED.contains(&port.as_str()))
            .map(|(from, _, _)| *from)
            .collect();
        while let Some(node) = stack.pop() {
            if !seen.contains(&node) {
                seen.push(node);
                stack.extend(data.iter().filter(|(_, to, _)| *to == node).map(|(from, _, _)| *from));
            }
        }
        let found: Vec<u64> = seen.into_iter().filter(|node| boards.contains(node)).collect();
        if let [board] = found[..] {
            links.push((board, writer));
        }
    }
    if links.is_empty() {
        return;
    }
    let Some(edges) = graph.as_object_mut().map(|g| g.entry("edges").or_insert_with(|| json!([]))) else { return };
    let Some(edges) = edges.as_array_mut() else { return };
    for (board, writer) in links {
        edges.push(json!({
            "from": { "node": board, "role": "DataStreamOut", "port": "time" },
            "to": { "node": writer, "role": "DataStreamIn", "port": "time" },
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

    fn data(from: u64, out: &str, to: u64, input: &str) -> Value {
        json!({ "from": end(from, "DataStreamOut", out), "to": end(to, "DataStreamIn", input) })
    }

    fn patched(project: Value) -> Value {
        let mut config = IncomingConfig { versions: Versions::new(), json: project };
        super::PATCH.apply_to("std", &mut config);
        config.json
    }

    fn time(board: u64, writer: u64) -> Value {
        data(board, "time", writer, "time")
    }

    /// A writer whose spikes come from a board through other nodes, or whose digital inputs come straight from one,
    /// reads that board's time; one with only positions, one with its time connected, and one fed by two boards don't.
    #[test]
    fn connects_the_board_feeding_write_ltx() {
        let project = json!({ "graph": {
            "nodes": {
                "1": { "node": { "type": "AcquisitionBoard", "settings": {} } },
                "2": { "node": { "type": "SpikeDetector", "settings": {} } },
                "3": { "node": { "type": "WriteLtx", "settings": {} } },
                "4": { "node": { "type": "WriteLtx", "settings": {} } },
                "5": { "node": { "type": "WriteLtx", "settings": {} } },
                "6": { "node": { "type": "AcquisitionBoard", "settings": {} } },
                "7": { "node": { "type": "WriteLtx", "settings": {} } },
                "8": { "node": { "type": "PositionDetector", "settings": {} } },
                "9": { "node": { "type": "WriteLtx", "settings": {} } },
            },
            "edges": [
                data(1, "headstages", 2, "continuous"),
                data(2, "spikes", 3, "spikes"),
                data(6, "digital", 5, "events"),
                time(6, 5),
                data(1, "digital", 7, "events"),
                data(8, "positions", 9, "positions"),
            ],
            "disabled_edges": [data(6, "digital", 4, "events"), data(2, "spikes", 7, "spikes")],
        } });
        let patched = patched(project);
        let edges = patched["graph"]["edges"].as_array().unwrap();
        assert!(edges.contains(&time(1, 3)), "through the detector");
        assert!(edges.contains(&time(6, 4)), "by a disabled edge");
        assert_eq!(edges.iter().filter(|e| e["to"]["node"] == 5).count(), 2, "its own left as it was");
        assert!(
            !edges.iter().any(|e| e["to"]["node"] == 9 && e["to"]["port"] == "time"),
            "nothing by the board's clock"
        );
        // Node 7: digital inputs from board 1, but spikes (disabled) also from board 1, so one board: connected.
        assert!(edges.contains(&time(1, 7)));
    }

    #[test]
    fn two_boards_is_too_many_to_choose_from() {
        let project = json!({ "graph": {
            "nodes": {
                "1": { "node": { "type": "AcquisitionBoard", "settings": {} } },
                "2": { "node": { "type": "AcquisitionBoard", "settings": {} } },
                "3": { "node": { "type": "WriteLtx", "settings": {} } },
            },
            "edges": [data(1, "digital", 3, "events"), data(2, "headstages", 3, "eeg")],
        } });
        let patched = patched(project);
        assert_eq!(patched["graph"]["edges"].as_array().unwrap().len(), 2);
    }
}
