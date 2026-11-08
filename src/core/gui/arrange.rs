//! Arranging the graph's nodes: a layered layout (Sugiyama's method, as dagre does it, through `dugong`), left to
//! right the way data flows. Nodes go in columns by how far along the data's flow they are, ordered within each
//! to cross as few edges as can be, and placed to keep edges short and straight.
//!
//! Sources go in the first column, and writers in the last (viewers go just after what they view). Settings edges
//! count too, though for less than data edges, the other way round (from the node whose setting it is to the one
//! editing or reading it), as a viewer editing a detector's settings is usually after it anyway. But a settings edge
//! to or from a node without data edges (Run Command, say, which a writer triggers) is all there is to place that node
//! by, so it counts just as a data edge would. Whether an edge is enabled doesn't matter: the layout's the same either
//! way.

use std::collections::HashMap;

use dugong::graphlib::{Graph as Layered, GraphOptions};
use dugong::{EdgeLabel, GraphLabel, NodeLabel, RankDir};

use super::canvas::node_size;
use crate::core::engine::{Dimension, Graph, NodeId, NodesEnum};

/// Room between nodes in a column, and between columns.
const NODE_GAP: f64 = 36.0;
const COLUMN_GAP: f64 = 90.0;
/// The hidden start node, and how hard it pulls.
const START: &str = "start";
const START_WEIGHT: f64 = 4.0;
/// How hard edges pull the nodes they join together: settings edges much less than data.
const DATA_WEIGHT: f64 = 1.0;
const SETTINGS_WEIGHT: f64 = 0.25;

/// Where each node goes, its top-left corner, with the whole arrangement's top-left where the graph's was.
pub fn arrange<N: NodesEnum>(graph: &Graph<N>) -> HashMap<NodeId, [f32; 2]> {
    let resolved = graph.resolve();
    let key = |id: NodeId| id.0.to_string();
    let mut layered: Layered<NodeLabel, EdgeLabel, GraphLabel> = Layered::new(GraphOptions::default());
    layered.set_graph(GraphLabel {
        rankdir: RankDir::LR,
        nodesep: NODE_GAP,
        ranksep: COLUMN_GAP,
        ..Default::default()
    });
    let mut sizes = HashMap::new();
    for (id, _) in graph.iter() {
        let Some(state) = resolved.get(&id) else { continue };
        let size = node_size(state);
        sizes.insert(id, size);
        let label = NodeLabel { width: size.x as f64, height: size.y as f64, ..Default::default() };
        layered.set_node(key(id), label);
    }
    // Nodes with data edges (enabled or not); a settings edge to or from one without counts as a data edge.
    let has_data: std::collections::HashSet<NodeId> = graph
        .edges()
        .iter()
        .filter(|e| e.dimension == Dimension::Data)
        .flat_map(|e| [e.from.node, e.to.node])
        .collect();
    let as_data = |edge: &crate::core::engine::Edge| {
        edge.dimension == Dimension::Data || !has_data.contains(&edge.from.node) || !has_data.contains(&edge.to.node)
    };
    // One edge each way between a pair of nodes, weighted by how many there are (settings counting for less).
    let mut weights: HashMap<(NodeId, NodeId), f64> = HashMap::new();
    for edge in graph.edges() {
        let (from, to, weight) = match as_data(edge) {
            true => (edge.from.node, edge.to.node, DATA_WEIGHT),
            false => (edge.to.node, edge.from.node, SETTINGS_WEIGHT),
        };
        if from != to && sizes.contains_key(&from) && sizes.contains_key(&to) {
            *weights.entry((from, to)).or_default() += weight;
        }
    }
    // Nodes something leads to, which aren't placed as sources.
    let led_to: std::collections::HashSet<NodeId> = weights.keys().map(|&(_, to)| to).collect();
    let mut pairs: Vec<_> = weights.into_iter().collect();
    pairs.sort_by_key(|((a, b), _)| (a.0, b.0)); // the same arrangement every time
    // Nodes without inputs that nothing leads to (sources) are pulled to the first column, by edges from a start node
    // that isn't shown. Not one a settings edge counting as data leads to (Run Command): it couldn't go further left
    // anyway, and the pull, a long edge across every column, would drag it up or down out of line with what leads to it.
    let edge = |weight| EdgeLabel { weight, minlen: 1, ..Default::default() };
    layered.set_node(START, NodeLabel { width: 1.0, height: 1.0, ..Default::default() });
    let mut ids: Vec<NodeId> = sizes.keys().copied().collect();
    ids.sort_by_key(|id| id.0);
    for id in ids {
        if graph.get(id).is_some_and(|e| e.node.inputs().is_empty()) && !led_to.contains(&id) {
            layered.set_edge_with_label(START, key(id), edge(START_WEIGHT));
        }
    }
    for ((from, to), weight) in pairs {
        layered.set_edge_with_label(key(from), key(to), edge(weight));
    }
    if let Err(e) = dugong::layout(&mut layered) {
        log::warn!("Couldn't arrange the graph: {e:?}");
        return HashMap::new();
    }

    // dagre gives centres; nodes are placed by their top-left corners.
    let mut placed: HashMap<NodeId, [f32; 2]> = sizes
        .iter()
        .filter_map(|(&id, size)| {
            let label = layered.node(&key(id))?;
            Some((id, [label.x? as f32 - size.x / 2.0, label.y? as f32 - size.y / 2.0]))
        })
        .collect();
    let corner = |positions: &mut dyn Iterator<Item = [f32; 2]>| {
        positions.fold([f32::INFINITY; 2], |c, p| [c[0].min(p[0]), c[1].min(p[1])])
    };
    let before = corner(&mut graph.iter().filter(|(id, _)| placed.contains_key(id)).map(|(_, e)| e.pos));
    let after = corner(&mut placed.values().copied());
    for pos in placed.values_mut() {
        *pos = [pos[0] - after[0] + before[0], pos[1] - after[1] + before[1]];
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Nodes;
    use crate::core::engine::{Port, PortRole};

    #[test]
    fn data_flows_left_to_right_in_columns() {
        let mut graph = Graph::<Nodes>::default();
        let add = |graph: &mut Graph<Nodes>, name: &str, pos: [f32; 2]| {
            let index = Nodes::TYPES.iter().position(|t| t.name == name).unwrap();
            graph.add(Nodes::create(index), pos)
        };
        // All in a heap, the viewer before the detector.
        let viewer = add(&mut graph, "Spike Viewer", [0.0, 0.0]);
        let board = add(&mut graph, "Acquisition Board", [10.0, 0.0]);
        let detector = add(&mut graph, "Spike Detector", [20.0, 0.0]);
        let lfp = add(&mut graph, "LFP Viewer", [30.0, 0.0]);
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(board, PortRole::Output, 0), port(detector, PortRole::Input, 0)).unwrap();
        graph.connect(port(detector, PortRole::Output, 0), port(viewer, PortRole::Input, 0)).unwrap();
        graph.connect(port(board, PortRole::Output, 0), port(lfp, PortRole::Input, 0)).unwrap();
        graph.connect(port(viewer, PortRole::Control, 0), port(detector, PortRole::Setting, 0)).unwrap();
        let placed = arrange(&graph);
        let x = |id| placed[&id][0];
        assert!(x(board) < x(detector) && x(detector) < x(viewer), "{placed:?}");
        assert_eq!(x(board), 0.0, "where the graph was");
        // The LFP Viewer's just after the board, beside the detector, not overlapping it.
        let (d, l) = (placed[&detector], placed[&lfp]);
        assert_eq!(l[0], d[0]);
        let resolved = graph.resolve();
        let height = |id| node_size(&resolved[&id]).y;
        assert!(d[1] + height(detector) <= l[1] || l[1] + height(lfp) <= d[1], "{d:?} {l:?}");
    }

    /// Run Command has only a settings edge, from the writer that triggers it: it goes after the writer, not first as
    /// if it were a source, and the same whether the edge is enabled or not.
    #[test]
    fn a_node_with_only_settings_edges_is_placed_by_them() {
        let mut graph = Graph::<Nodes>::default();
        let add = |graph: &mut Graph<Nodes>, name: &str| {
            let index = Nodes::TYPES.iter().position(|t| t.name == name).unwrap();
            graph.add(Nodes::create(index), [0.0, 0.0])
        };
        let command = add(&mut graph, "Run Command");
        let board = add(&mut graph, "Acquisition Board");
        let detector = add(&mut graph, "Spike Detector");
        let writer = add(&mut graph, "Write LTX");
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(board, PortRole::Output, 0), port(detector, PortRole::Input, 0)).unwrap();
        graph.connect(port(detector, PortRole::Output, 0), port(writer, PortRole::Input, 0)).unwrap();
        graph.connect(port(writer, PortRole::Control, 4), port(command, PortRole::Setting, 0)).unwrap();
        let placed = arrange(&graph);
        let x = |id| placed[&id][0];
        assert!(x(board) < x(detector) && x(detector) < x(writer) && x(writer) < x(command), "{placed:?}");
        let edge = graph.edge_into(port(command, PortRole::Setting, 0)).unwrap();
        graph.set_edge_disabled(edge, true);
        assert_eq!(arrange(&graph), placed, "disabled");
    }

    /// In the example rig, Run Command goes beside the writer that triggers it, above the Alarm Viewer (which goes beside
    /// the Alarm Processor, below the writer), so the edges don't cross.
    #[test]
    fn a_node_placed_by_a_settings_edge_stays_in_line_with_what_leads_to_it() {
        let mut graph = crate::example_graph();
        let find = |graph: &Graph<Nodes>, label: &str| graph.iter().find(|(_, e)| e.label == label).unwrap().0;
        let index = Nodes::TYPES.iter().position(|t| t.name == "Run Command").unwrap();
        let command = graph.add(Nodes::create(index), [0.0, 0.0]);
        let writer = find(&graph, "Write LTX");
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(writer, PortRole::Control, 4), port(command, PortRole::Setting, 0)).unwrap();
        let placed = arrange(&graph);
        let (alarms, processor) = (find(&graph, "Alarm Viewer"), find(&graph, "Alarm Processor"));
        assert!(placed[&writer][1] < placed[&processor][1], "as the rig is: {placed:?}");
        assert_eq!(placed[&command][0], placed[&alarms][0], "the same column");
        assert!(placed[&command][1] < placed[&alarms][1], "{placed:?}");
    }
}
