//! The Channel Selector: selects some of a continuous stream's channels (such as "1-4, 9"), perhaps none, and passes on
//! those while the selection's on, or all of them while it's off. The selection's a setting, which an LFP Viewer downstream
//! can choose: look through all the channels, click the numbers of the ones to keep, then apply it.

use crate::core::engine::TreeShake;
use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Input, Node, NodeInfo, Output, Pass, PassState, SettingPort, UiContext};
use crate::standard_components::ephys::reference::{channels_ui, parse_channels};
use crate::standard_components::ephys::{ChannelSelection, Continuous};
use crate::standard_components::widgets::{inline_block, input_summary, power, require_input};
use crate::{Buffer, DataStream, Edges, Kind, Setting};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelSelector {
    /// Whether only the selected channels are passed on (otherwise all are).
    pub apply: bool,
    /// The channels selected, such as "all" or "1-4, 9" (counting from 1).
    pub channels: String,
}

impl Default for ChannelSelector {
    fn default() -> Self {
        ChannelSelector { apply: false, channels: "all".into() }
    }
}

impl ChannelSelector {
    /// The channels passed on, of `count`: the selection, perhaps none, if it's applied and makes sense, else all.
    pub fn passed(&self, count: u32) -> Vec<u32> {
        match parse_channels(&self.channels, count) {
            Ok(channels) if self.apply => channels,
            _ => (0..count).collect(),
        }
    }
}

/// The channels passed on, as of the settings it was worked out from.
#[derive(Default)]
pub struct Passed {
    from: Option<(bool, String)>,
    channels: Vec<u32>,
}

impl PassState for Passed {
    const WHY: &'static str = "The channels passed on, worked out from the settings.";
    const CARRIED_OVER: bool = false;
}

impl Node<Edges> for ChannelSelector {
    const INFO: NodeInfo = NodeInfo {
        name: "Channel Selector",
        category: Category::Processing,
        description: "Select individual channels, then switch the selection on or off as a whole: while it's off, \
                      all the channels are passed on.",
    };

    /// Only while it's given something, and something needs what it makes.
    fn evaluate_tree_shake(&self, inputs: &[bool], outputs: &[bool]) -> TreeShake {
        TreeShake::needing_all(inputs.contains(&true) && outputs.contains(&true), inputs.len())
    }

    type State = Passed;
    type Shared = ();

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }]
    }

    fn outputs(&self, inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let stream = match inputs[0] {
            Some(DataStream::Continuous(c)) => {
                let channels = self.passed(c.channels).len() as u32;
                Some(DataStream::Continuous(Continuous { channels, ..c }))
            }
            _ => None,
        };
        vec![Output { name: "selected", kind: Kind::Continuous, stream }]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        let mut warnings = require_input(inputs, "a continuous data stream");
        if let Some(DataStream::Continuous(c)) = inputs[0]
            && let Err(e) = parse_channels(&self.channels, c.channels)
        {
            warnings.push(format!("{e}: passing on all the channels"));
        }
        warnings
    }

    fn initial_state(&self, _inputs: &[Option<DataStream>]) -> Passed {
        Passed::default()
    }

    fn process(&self, pass: &mut Pass<Edges>, passed: &mut Passed) {
        let Some(Some(Buffer::Continuous(input))) = pass.inputs.first() else { return };
        let Some(Buffer::Continuous(output)) = pass.outputs.get_mut(0) else { return };
        // Worked out again only when the settings change.
        if passed.from.as_ref().is_none_or(|(apply, channels)| *apply != self.apply || *channels != self.channels) {
            passed.channels = self.passed(input.channels as u32);
            passed.from = Some((self.apply, self.channels.clone()));
        }
        output.len = input.len.min(output.capacity);
        for (to, &from) in passed.channels.iter().take(output.channels).enumerate() {
            let len = output.len;
            output.channel_mut(to).copy_from_slice(&input.channel(from as usize)[..len]);
        }
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        let selection = ChannelSelection { apply: self.apply, channels: self.channels.clone() };
        vec![SettingPort { name: "selection", value: selection.into(), on_right: true }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::ChannelSelection(selection) = value {
            (self.apply, self.channels) = (selection.apply, selection.channels);
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        let count = match context.inputs[0] {
            Some(DataStream::Continuous(c)) => c.channels,
            _ => 64,
        };
        let apply = &mut self.apply;
        channels_ui(ui, "Selected", &mut self.channels, count, |ui, selected| {
            // Together, on the next line if they won't fit on this one.
            inline_block(ui, "apply", false, |ui| {
                power(ui, apply, "Apply selection");
                if *apply {
                    ui.label("Selection applied");
                } else if selected < count as usize {
                    let text = if selected == 0 { "Nothing selected" } else { "Selection pending" };
                    ui.colored_label(ui.visuals().error_fg_color, text);
                }
            });
        });
        ui.add_space(4.0);
        input_summary(ui, context.inputs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::{DataStreamType, PASS_DURATION};

    #[test]
    fn passes_on_the_selected_channels_once_applied() {
        let stream = Continuous { channels: 4, sample_rate: 1000.0 };
        let mut selector = ChannelSelector { apply: false, channels: "2, 4".into() };
        let run = |selector: &ChannelSelector| {
            let Some(DataStream::Continuous(out)) = selector.outputs(&[Some(DataStream::Continuous(stream))])[0].stream
            else {
                unreachable!()
            };
            let mut input = stream.buffer(PASS_DURATION);
            input.len = 3;
            for c in 0..4 {
                input.channel_mut(c).fill(c as f32);
            }
            let input = Buffer::Continuous(input);
            let mut outputs = [Buffer::Continuous(out.buffer(PASS_DURATION))];
            let inputs = [Some(&input)];
            let mut pass = Pass::new(0, PASS_DURATION, &inputs, &mut outputs);
            selector.process(&mut pass, &mut Passed::default());
            let Buffer::Continuous(output) = &outputs[0] else { unreachable!() };
            (0..output.channels).map(|c| output.channel(c)[2]).collect::<Vec<_>>()
        };
        assert_eq!(run(&selector), vec![0.0, 1.0, 2.0, 3.0], "all, until applied");
        selector.apply = true;
        assert_eq!(run(&selector), vec![1.0, 3.0]);
        selector.channels = "none".into();
        assert_eq!(run(&selector), Vec::<f32>::new(), "nothing, if that's what's selected");
        assert!(selector.warnings(&[Some(DataStream::Continuous(stream))]).is_empty());
    }

    /// The rig with nothing selected, and every kind of node that takes continuous data taking that, and every kind that
    /// takes what those make of it: all of it runs on real samples, records and draws, with no channels at all.
    #[test]
    fn nothing_selected_runs_records_and_draws_everything_after_it() {
        use crate::core::base_directory::Place;
        use crate::core::engine::{Graph, NodesEnum, Port, PortRole, Recording, Runner, Snapshot};
        use crate::standard_components::ephys::board::{Source, serve_test_board};
        use crate::standard_components::themes::THEMES;
        use crate::{Nodes, example_graph};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let mut graph = example_graph();
        let ids: Vec<_> = graph.ids().collect();
        let of_type = |graph: &Graph<Nodes>, name: &str| {
            ids.iter().copied().find(|id| graph.get(*id).is_some_and(|e| e.node.info().name == name)).unwrap()
        };
        let selector = of_type(&graph, "Channel Selector");
        if let Some(Nodes::ChannelSelector(s)) = graph.get_mut(selector).map(|e| &mut e.node) {
            (s.apply, s.channels) = (true, "none".into());
        }
        let board = of_type(&graph, "Acquisition Board");
        if let Some(Nodes::AcquisitionBoard(b)) = graph.get_mut(board).map(|e| &mut e.node) {
            b.source = Source::Fake { address: serve_test_board() };
        }
        // A node of each type taking what `from` has, of the kinds in `kinds`.
        let takers = |graph: &mut Graph<Nodes>, from: &[(Kind, Port)]| {
            let mut added = Vec::new();
            for index in 0..Nodes::TYPES.len() {
                let node = Nodes::create(index);
                let inputs: Vec<_> = (node.inputs().iter().enumerate())
                    .filter_map(|(input, i)| from.iter().find(|(kind, _)| *kind == i.kind).map(|f| (input, f.1)))
                    .collect();
                if !inputs.is_empty() {
                    let id = graph.add(node, [0.0, 0.0]);
                    added.push(id);
                    for (input, port) in inputs {
                        graph.connect(port, Port { node: id, role: PortRole::Input, index: input }).unwrap();
                    }
                }
            }
            added
        };
        let selected = Port { node: selector, role: PortRole::Output, index: 0 };
        let first = takers(&mut graph, &[(Kind::Continuous, selected)]);
        let resolved = graph.resolve();
        let made: Vec<(Kind, Port)> = (first.iter())
            .flat_map(|&node| {
                let outputs = resolved[&node].outputs.iter().enumerate();
                outputs.map(move |(index, o)| (o.kind, Port { node, role: PortRole::Output, index }))
            })
            .filter(|(kind, _)| matches!(kind, Kind::Spikes | Kind::Spectra))
            .collect();
        assert!(made.len() >= 2, "some spikes and spectra, of no channels");
        takers(&mut graph, &made);
        let resolved = graph.resolve();
        let stream = &resolved[&selector].outputs[0].stream;
        assert!(matches!(stream, Some(DataStream::Continuous(c)) if c.channels == 0));

        let dir = std::env::temp_dir().join(format!("ephys-rs-nothing-selected-{}", std::process::id()));
        let snapshot = Snapshot::new(0, graph.clone(), None);
        let runner = Runner::start(&snapshot, 2);
        let place = Place { directory: dir.clone(), prefix: Some("trial a".into()) };
        let recording =
            snapshot.with_recording(1, Some(Arc::new(Recording::prepare(&snapshot, Some(place), &Default::default()))));
        assert!(runner.update(&recording), "recording carries on the same run");
        let deadline = Instant::now() + Duration::from_secs(10);
        while runner.clock() < 0.5 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(runner.clock() >= 0.5, "samples came through, to {} s", runner.clock());

        for id in graph.ids().collect::<Vec<_>>() {
            let inputs = resolved[&id].input_streams.clone();
            let mut graph = graph.clone();
            let shared = runner.shared();
            let shared = Some(&*shared);
            let mut harness = egui_kittest::Harness::builder().with_size([800.0, 600.0]).build_ui(|ui| {
                graph.edit(id, &inputs, &THEMES[0].colours, shared, None, true, false, None, |node, context| {
                    node.settings_ui(ui, context);
                    if node.has_custom_ui() {
                        node.custom_ui(ui, context);
                    }
                });
            });
            harness.run_steps(2);
        }

        runner.update(&recording.with_recording(2, None));
        drop(runner);
        crate::core::background::wait();
        let mut files = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path());
        let tetrode = files.find(|p| p.extension().is_some_and(|x| x == "1")).expect("the spikes' file");
        assert!(std::fs::metadata(tetrode).unwrap().len() > 400, "the spikes, not selected, have been recorded");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The channels are chosen from a checkbox each, and one for all of them, as a Reference's are.
    #[test]
    fn its_settings_are_a_checkbox_for_each_channel() {
        use crate::core::engine::{Graph, NodesEnum};
        use crate::standard_components::themes::THEMES;
        use egui_kittest::kittest::Queryable as _;
        let mut harness = egui_kittest::Harness::builder().with_size([340.0, 220.0]).wgpu().build_ui_state(
            |ui, selector: &mut ChannelSelector| {
                let mut graph = Graph::<crate::Nodes>::default();
                let index = crate::Nodes::TYPES.iter().position(|t| t.name == "Channel Selector").unwrap();
                let id = graph.add(crate::Nodes::create(index), [0.0, 0.0]);
                let stream = Continuous { channels: 16, sample_rate: 30000.0 };
                let inputs = [Some(DataStream::Continuous(stream))];
                graph.edit(id, &inputs, &THEMES[0].colours, None, None, false, false, None, |_, context| {
                    selector.settings_ui(ui, context)
                });
            },
            ChannelSelector { apply: false, channels: "1-4".into() },
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.get_by_label("Selected 4 of the 16 channels");
        harness.get_by_label("Selection pending");
        harness.snapshot("channel_selector_settings");
        harness.get_by_label(" 9").click();
        harness.run_steps(2);
        assert_eq!(harness.state().channels, "1-4, 9");
        harness.get_by_label("All").click();
        harness.run_steps(2);
        assert_eq!(harness.state().channels, "all");
        assert!(harness.query_by_label("Selection pending").is_none(), "nothing left out");
        // Narrower with nothing to say, the power button moves up beside All, which takes a frame more.
        harness.run_steps(2);
        harness.get_by_label("Apply selection").click();
        harness.run_steps(2);
        assert!(harness.state().apply);
        harness.get_by_label("Selection applied");
    }
}
