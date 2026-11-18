//! The Acquisition Board node: samples from a board's headstages, and its auxiliary, analogue and digital
//! inputs, acquired on a thread of its own (see `capture`) into a pool of buffers, which each pass swaps for its
//! own.

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use super::capture::{self, Capture, Shape, State, Status};
use super::{AUX_PER_HEADSTAGE, Bandwidth, BoardInfo, Config, Headstage, OpalKellyKind, PORTS, SAMPLE_RATES, Source};
use crate::standard::ephys::{Continuous, Events};
use crate::standard::pool::Pool;
use crate::standard::widgets::{named_settings_grid, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind};
use ephys_rs::engine::{
    Category, Flow, Node, NodeInfo, NodeThread, PassContext, PoolStatus, Port, Prepare, PrepareContext, Prepared,
    Suggestion, ThreadContext, UiContext, UserDecision,
};
use ephys_rs::gui::widgets::{Icon, small_icon_button};

/// The board's analogue inputs.
pub const ADC_CHANNELS: u32 = 8;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AcquisitionBoard {
    pub source: Source,
    pub sample_rate: u32,
    /// The headstages expected: their channels are the headstage output's. Detect finds a board's.
    pub headstages: Vec<Headstage>,
    /// Whether to output the headstages' auxiliary inputs, and the board's analogue inputs.
    pub aux: bool,
    pub adc: bool,
    pub bandwidth: Bandwidth,
}

impl Default for AcquisitionBoard {
    fn default() -> Self {
        AcquisitionBoard {
            source: Source::default(),
            sample_rate: 30000,
            headstages: vec![
                Headstage { port: "A1".into(), channels: 32 },
                Headstage { port: "B1".into(), channels: 32 },
            ],
            aux: false,
            adc: false,
            bandwidth: Bandwidth::default(),
        }
    }
}

/// Acquiring, while running.
#[derive(Default)]
pub struct Acquiring {
    capture: Option<Capture>,
}

impl ephys_rs::engine::PassState for Acquiring {
    const WHY: &'static str = "The board's capture thread and its pool of buffers: each pass takes the samples \
                               that have come since the last, in order.";
}

/// How acquisition is going, for the node's window.
#[derive(Default)]
pub struct BoardDisplay {
    status: Status,
    pool: PoolStatus,
}

/// How long to wait for a board to start, as the node prepares: opening one loads its FPGA, which takes a few seconds.
const START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl AcquisitionBoard {
    /// What isn't as set about the board it's acquiring from, as it's started: that it started; its headstages are on
    /// the ports set (taking on others would label channels as ones they aren't); and its bandwidth's what's set (the
    /// chips can only do certain values). Each with the whole fix.
    fn decisions(&self, status: &Status) -> Vec<UserDecision<Self>> {
        match &status.state {
            State::Running => {}
            State::Problem(problem) => return vec![UserDecision { what: problem.clone(), suggestion: None }],
            State::Starting => {
                let what = format!("the board hadn't started after {} s", START_TIMEOUT.as_secs());
                return vec![UserDecision { what, suggestion: None }];
            }
        }
        let (mut fixed, mut whats) = (self.clone(), Vec::new());
        if let Some(info) = &status.info
            && !Headstage::readable(&self.headstages, &info.headstages)
        {
            let (found, set) = (Headstage::describe(&info.headstages), Headstage::describe(&self.headstages));
            whats.push(format!("the board's headstages are {found}, not the {set} set"));
            fixed.headstages = info.headstages.clone();
        }
        if let Some(got) = status.bandwidth
            && !got.near(&self.bandwidth)
        {
            let dsp = got.dsp_hz.map_or(String::new(), |hz| format!(", DSP high-pass {hz:.2} Hz"));
            whats.push(format!(
                "the chips' bandwidth is {:.2} to {:.0} Hz{dsp}, as near as they get to what's set",
                got.lower_hz, got.upper_hz
            ));
            fixed.bandwidth = got;
        }
        let suggestion = Suggestion { label: "Use the hardware's settings".into(), node: fixed };
        whats.into_iter().map(|what| UserDecision { what, suggestion: Some(suggestion.clone()) }).collect()
    }

    fn config(&self) -> Config {
        Config { sample_rate: self.sample_rate, bandwidth: self.bandwidth, headstages: self.headstages.clone() }
    }

    /// The outputs' indices: headstages and digital inputs, then auxiliary and analogue inputs if they're on.
    fn aux_output(&self) -> Option<usize> {
        self.aux.then_some(2)
    }

    fn adc_output(&self) -> Option<usize> {
        self.adc.then_some(2 + self.aux as usize)
    }
}

impl Node<Edges> for AcquisitionBoard {
    const INFO: NodeInfo = NodeInfo {
        name: "Acquisition Board",
        category: Category::Source,
        description: "Headstage channels, and auxiliary, analogue and digital inputs, from an acquisition board: \
                      an Open Ephys board, Intan's RHD USB board, or the fake board in synthetic_streams/.",
    };

    type State = Acquiring;
    type WorkerState = ();
    /// What it shows, for its window.
    type Shared = Pool<BoardDisplay>;
    type UiState = ();
    type GlobalSettings = super::OpenEphysSettings;

    fn list_ports_data_stream_out(&self, _inputs: &[Option<DataStream>]) -> Vec<Port<Edges>> {
        let rate = self.sample_rate as f32;
        let continuous = |channels| Some(DataStream::Continuous(Continuous { channels, sample_rate: rate }));
        let mut outputs = vec![
            Port::DataStreamOut {
                name: "headstages",
                data_stream_kind: Kind::Continuous,
                stream: continuous(Headstage::total(&self.headstages)),
            },
            Port::DataStreamOut {
                name: "digital",
                data_stream_kind: Kind::Events,
                stream: Some(DataStream::Events(Events { lines: self.source.ttl_lines(), sample_rate: rate })),
            },
        ];
        if self.aux {
            let channels = AUX_PER_HEADSTAGE * self.headstages.len() as u32;
            outputs.push(Port::DataStreamOut {
                name: "auxiliary",
                data_stream_kind: Kind::Continuous,
                stream: continuous(channels),
            });
        }
        if self.adc {
            outputs.push(Port::DataStreamOut {
                name: "analogue",
                data_stream_kind: Kind::Continuous,
                stream: continuous(ADC_CHANNELS),
            });
        }
        outputs
    }

    fn threads(&self) -> Vec<NodeThread> {
        vec![NodeThread {
            name: "Board capture",
            link: "buffer pool",
            flow: Flow::ToPasses,
            why: "On a thread of its own so samples are taken from the board as they come (before its buffer \
                  overflows), whatever the passes are doing.",
            swapped_outputs: (0..self.list_ports_data_stream_out(&[]).len()).collect(),
        }]
    }

    fn thread_ui(&self, _thread_idx: usize, ui: &mut egui::Ui, context: &ThreadContext) {
        let small = |text: String| RichText::new(text).size(11.0);
        let pool = context.shared::<Pool<BoardDisplay>>();
        let Some(display) = pool.and_then(Pool::newest) else {
            ui.label(small("Reads the board as samples arrive; starts when running.".into()).weak());
            return;
        };
        let status = &display.status;
        match &status.state {
            State::Starting => ui.label(small("Opening the board…".into()).weak()),
            State::Running => ui.label(small(status.info.as_ref().map_or(String::new(), |i| i.description.clone()))),
            State::Problem(problem) => ui.label(small(problem.clone()).color(context.theme.warning)),
        };
        ui.label(small(format!("{:.0} samples/s · {} samples", status.rate, status.samples)).weak());
        if status.board_dropped + status.not_taken > 0 {
            let text = format!("{} dropped by the board, {} not taken in time", status.board_dropped, status.not_taken);
            ui.label(small(text).color(context.theme.warning));
        }
    }

    fn thread_pool(&self, _thread_idx: usize, context: &ThreadContext) -> Option<PoolStatus> {
        context.shared::<Pool<BoardDisplay>>().and_then(|l| l.newest().map(|d| d.pool))
    }

    /// Starts acquiring, carrying on with the board if it's acquiring as set already (see `Capture::start`): its
    /// capture thread opens the board, which is waited for, once every node's been asked, to say what isn't as set
    /// (see `decisions`).
    fn prepare_for_snapshot(&self, context: &PrepareContext<Edges, Self>) -> Prepare<Acquiring, (), Self> {
        if context.unchanged(self) {
            return Prepared::nothing().into();
        }
        let rate = self.sample_rate as f32;
        let continuous = |channels| Continuous { channels, sample_rate: rate };
        let shape = Shape {
            ephys: continuous(Headstage::total(&self.headstages)),
            aux: continuous(AUX_PER_HEADSTAGE * self.headstages.len() as u32),
            adc: continuous(ADC_CHANNELS),
            events: Events { lines: self.source.ttl_lines(), sample_rate: rate },
        };
        let capture = Capture::start(self.source.clone(), self.config(), shape);
        let board = self.clone();
        Prepare::Wait(Box::new(move || {
            let decisions = board.decisions(&capture.started(START_TIMEOUT));
            Prepared::shared(Acquiring { capture: Some(capture) }).with_decisions(decisions)
        }))
    }

    /// Takes the samples that have arrived since the last pass, swapping the buffers they're in for the pass's
    /// own (which go back to the capture thread's pool to fill), so nothing's copied.
    fn process(&self, pass: &mut PassContext<Edges>, acquiring: &mut Acquiring, _: &mut ()) {
        let Some(capture) = &acquiring.capture else { return };
        // The headstages', auxiliary and analogue inputs' outputs, if they're on.
        let outputs = [Some(0), self.aux_output(), self.adc_output()];
        match capture.pool().take(|samples| samples.is_empty()) {
            Some(mut samples) => {
                let buffers = [&mut samples.ephys, &mut samples.aux, &mut samples.adc];
                for (buffer, output) in buffers.into_iter().zip(outputs) {
                    if let Some(Some(Buffer::Continuous(out))) = output.map(|o| pass.outputs.get_mut(o)) {
                        std::mem::swap(out, buffer);
                    }
                }
                if let Some(Buffer::Events(out)) = pass.outputs.get_mut(1) {
                    std::mem::swap(out, &mut samples.events);
                }
                capture.pool().give_back(samples);
            }
            // Nothing's come since the last pass.
            None => {
                for output in outputs.into_iter().flatten() {
                    if let Some(Buffer::Continuous(out)) = pass.outputs.get_mut(output) {
                        out.len = 0;
                    }
                }
                if let Some(Buffer::Events(out)) = pass.outputs.get_mut(1) {
                    out.events.clear();
                    out.pass_samples = 0;
                }
            }
        }
        // Not if the capture thread's updating the status just then: the next pass shows it.
        if let Some(status) = capture.try_status() {
            pass.shared::<Pool<BoardDisplay>>().send_newest(pass.sequence, |display| {
                display.status.clone_from(&status);
                display.pool = capture.pool().status();
            });
        }
    }

    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<Edges>) {
        let Some(ui) = ui else { return };
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, context| {
            let theme = context.theme;
            settings_grid(ui, |ui| {
                setting(ui, "Board", |ui| source_ui(ui, &mut node.source));
                match &mut node.source {
                    Source::Fake { address } => {
                        setting(ui, "Address", |ui| ui.text_edit_singleline(address));
                    }
                    Source::OpalKelly { board } => {
                        setting(ui, "Kind", |ui| {
                            egui::ComboBox::from_id_salt("rhythm")
                                .selected_text(board.name())
                                .show_ui(ui, |ui| {
                                    for kind in [OpalKellyKind::OpenEphys, OpalKellyKind::IntanRhdUsb] {
                                        ui.selectable_value(board, kind, kind.name());
                                    }
                                })
                                .response
                        });
                    }
                    Source::Oni => {}
                }
                setting(ui, "Sample rate", |ui| {
                    egui::ComboBox::from_id_salt("rate")
                        .selected_text(format!("{} Hz", node.sample_rate))
                        .show_ui(ui, |ui| {
                            for rate in SAMPLE_RATES {
                                ui.selectable_value(&mut node.sample_rate, rate, format!("{rate} Hz"));
                            }
                        })
                        .response
                });
            });

            ui.add_space(6.0);
            ui.label(RichText::new("Headstages").strong());
            headstages_ui(ui, &mut node.headstages);
            detect_ui(ui, node, theme);

            ui.add_space(6.0);
            ui.checkbox(&mut node.aux, format!("Auxiliary inputs ({AUX_PER_HEADSTAGE} per headstage)"));
            ui.checkbox(&mut node.adc, format!("Analogue inputs ({ADC_CHANNELS})"));

            ui.add_space(6.0);
            ui.label(RichText::new("Bandwidth").strong());
            named_settings_grid(ui, "bandwidth", |ui| {
                let upper = node.bandwidth.upper_hz;
                setting(ui, "Low cut", |ui| ui.add(hz(&mut node.bandwidth.lower_hz, 0.1..=upper)));
                setting(ui, "High cut", |ui| ui.add(hz(&mut node.bandwidth.upper_hz, 100.0..=20000.0)));
                setting(ui, "DSP high-pass", |ui| {
                    ui.horizontal(|ui| {
                        let mut on = node.bandwidth.dsp_hz.is_some();
                        ui.checkbox(&mut on, "on");
                        node.bandwidth.dsp_hz = on.then(|| {
                            let mut cutoff = node.bandwidth.dsp_hz.unwrap_or(0.5);
                            ui.add(hz(&mut cutoff, 0.01..=1000.0));
                            cutoff
                        });
                    })
                    .response
                });
            });

            ui.separator();
            if !context.running {
                ui.weak("Press Play to start acquiring.");
            }
            let pool = context.shared::<Pool<BoardDisplay>>();
            let Some(display) = pool.and_then(Pool::newest) else { return };
            let status = &display.status;
            match &status.state {
                State::Starting if !context.running => {}
                State::Starting => {
                    ui.weak("Opening the board…");
                }
                State::Running => {
                    ui.label(status.info.as_ref().map_or("", |i| i.description.as_str()));
                }
                State::Problem(problem) => {
                    ui.label(RichText::new(problem).color(theme.warning));
                }
            }
            let rate = if context.running { format!("{:.0} samples/s · ", status.rate) } else { String::new() };
            ui.weak(format!(
                "{rate}{} samples · {} dropped by the board · {} not taken in time",
                status.samples, status.board_dropped, status.not_taken
            ));
            if status.mismatched > 0
                && let Some(info) = &status.info
            {
                let message = format!(
                    "{} samples skipped: the board has headstages {}, not the {} set.",
                    status.mismatched,
                    Headstage::describe(&info.headstages),
                    Headstage::describe(&node.headstages)
                );
                ui.label(RichText::new(message).color(theme.warning));
                if ui.button("Use the board's headstages").clicked() {
                    node.headstages = info.headstages.clone();
                }
            }
            if status.lost_events > 0 {
                let message = format!("{} digital input changes lost (too many in a pass)", status.lost_events);
                ui.label(RichText::new(message).color(theme.warning));
            }
            for warning in &status.warnings {
                ui.label(RichText::new(warning).small().color(theme.warning));
            }
        });
    }

    fn warnings(&self, _inputs: &[Option<DataStream>]) -> Vec<String> {
        if self.headstages.is_empty() {
            vec!["No headstages: add some, or detect the board's".into()]
        } else {
            Vec::new()
        }
    }
}

fn source_ui(ui: &mut egui::Ui, source: &mut Source) -> egui::Response {
    let name = |source: &Source| match source {
        Source::Fake { .. } => "Fake board (synthetic_streams/)",
        Source::OpalKelly { .. } => "Open Ephys gen 1/2, or Intan USB",
        Source::Oni => "Open Ephys gen 3",
    };
    let choices = [Source::default(), Source::OpalKelly { board: OpalKellyKind::OpenEphys }, Source::Oni];
    egui::ComboBox::from_id_salt("board")
        .selected_text(name(source))
        .show_ui(ui, |ui| {
            for choice in choices {
                let selected = std::mem::discriminant(source) == std::mem::discriminant(&choice);
                if ui.selectable_label(selected, name(&choice)).clicked() && !selected {
                    *source = choice;
                }
            }
        })
        .response
}

/// A frequency to edit.
fn hz(value: &mut f64, range: std::ops::RangeInclusive<f64>) -> egui::DragValue<'_> {
    egui::DragValue::new(value).range(range).speed(0.1).suffix(" Hz")
}

/// The headstages expected: a port and a channel count each.
fn headstages_ui(ui: &mut egui::Ui, headstages: &mut Vec<Headstage>) {
    let mut remove = None;
    egui::Grid::new("headstages").num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
        for (i, headstage) in headstages.iter_mut().enumerate() {
            let port = ui.label(format!("Port {}", headstage.port));
            egui::ComboBox::from_id_salt(("channels", i))
                .selected_text(format!("{} ch", headstage.channels))
                .show_ui(ui, |ui| {
                    for channels in [16, 32, 64] {
                        ui.selectable_value(&mut headstage.channels, channels, format!("{channels} ch"));
                    }
                })
                .response
                .labelled_by(port.id);
            if small_icon_button(ui, Icon::Delete, "Delete headstage", "Delete this headstage").clicked() {
                remove = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = remove {
        headstages.remove(i);
    }
    let order = |port: &str| PORTS.iter().position(|p| *p == port).unwrap_or(PORTS.len());
    // The next port's first MISO line, or failing that any free one.
    let free = |port: &&&str| headstages.iter().all(|h| h.port != **port);
    let next = PORTS.iter().step_by(2).find(free).or_else(|| PORTS.iter().find(free));
    if let Some(port) = next
        && ui.small_button("Add headstage").clicked()
    {
        headstages.push(Headstage { port: port.to_string(), channels: 32 });
        headstages.sort_by_key(|h| order(&h.port));
    }
}

type Detecting = std::sync::Arc<std::sync::Mutex<Option<Result<BoardInfo, String>>>>;

/// A button to find the board's headstages and use them; it asks the board on a thread of its own.
fn detect_ui(ui: &mut egui::Ui, board: &mut AcquisitionBoard, theme: &ephys_rs::theme::Colours) {
    let id = ui.scope_id().with("detect");
    let detecting: Option<Detecting> = ui.data(|d| d.get_temp(id));
    let answer = detecting.as_ref().and_then(|slot| slot.lock().unwrap().take());
    match (detecting, answer) {
        (Some(_), None) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Asking the board…");
            });
            ui.ctx().request_repaint();
        }
        (_, answer) => {
            if let Some(answer) = answer {
                ui.data_mut(|d| d.remove::<Detecting>(id));
                let result_id = id.with("result");
                let message = match answer {
                    Ok(info) => {
                        board.headstages = info.headstages.clone();
                        Ok(format!("{}: headstages {}", info.description, Headstage::describe(&info.headstages)))
                    }
                    Err(e) => Err(format!("Couldn't detect the board: {e}")),
                };
                ui.data_mut(|d| d.insert_temp(result_id, message));
            }
            if ui.button("Detect").on_hover_text("Ask the board which headstages it has, and use those").clicked() {
                ui.data_mut(|d| d.insert_temp(id, capture::detect(board.source.clone())));
            }
            match ui.data(|d| d.get_temp::<Result<String, String>>(id.with("result"))) {
                Some(Ok(message)) => {
                    ui.weak(message);
                }
                Some(Err(message)) => {
                    ui.label(RichText::new(message).color(theme.warning));
                }
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeType;
    use crate::Nodes;
    use crate::standard::themes::THEMES;
    use ephys_rs::engine::{Graph, NodesEnum};

    /// Waits a while for what about their hardware the nodes of the run's latest snapshot say someone should decide.
    fn decisions(runner: &ephys_rs::engine::Runner<Nodes>) -> Option<Vec<ephys_rs::engine::UserDecision<Nodes>>> {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < until {
            if let Some((_, decisions)) = runner.user_decisions() {
                return Some(decisions.into_iter().map(|(_, decision)| decision).collect());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    /// A board whose headstages aren't on the ports set is for someone to decide about (its channels would be taken for
    /// the ones set), with the board's as the fix; it runs meanwhile.
    #[test]
    fn headstages_not_as_set_are_to_be_decided_about() {
        let mut graph = Graph::<Nodes>::default();
        let board = graph.add(Nodes::create(NodeType::AcquisitionBoard), [0.0, 0.0]);
        if let Some(Nodes::AcquisitionBoard(b)) = graph.get_mut(board).map(|e| &mut e.node) {
            b.source = Source::Fake { address: super::super::serve_test_board() };
            b.headstages =
                vec![Headstage { port: "D1".into(), channels: 32 }, Headstage { port: "D2".into(), channels: 32 }];
        }
        let runner = ephys_rs::engine::Runner::start(&ephys_rs::engine::Snapshot::new(0, graph), 1);
        let found = decisions(&runner).expect("to decide");
        assert_eq!(found.len(), 1);
        let difference = &found[0];
        assert_eq!(difference.what, "the board's headstages are A1 32, B1 32, not the D1 32, D2 32 set");
        let suggestion = difference.suggestion.as_ref().expect("a suggestion");
        assert_eq!(suggestion.label, "Use the hardware's settings");
        let Nodes::AcquisitionBoard(fixed) = &suggestion.node else { panic!("the board") };
        assert_eq!(Headstage::describe(&fixed.headstages), "A1 32, B1 32");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while runner.stats().completed == 0 && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(runner.stats().completed > 0, "it runs meanwhile");
    }

    /// A board that can't be opened is to be decided about too, with no fix; without it (disabled), the rest runs.
    #[test]
    fn a_board_that_cant_be_opened_can_be_done_without() {
        let mut graph = Graph::<Nodes>::default();
        let board = graph.add(Nodes::create(NodeType::AcquisitionBoard), [0.0, 0.0]);
        let runner = ephys_rs::engine::Runner::start(&ephys_rs::engine::Snapshot::new(0, graph.clone()), 1);
        let found = decisions(&runner).expect("to decide");
        let whats: Vec<_> = found.iter().map(|d| &d.what).collect();
        assert!(found.iter().all(|d| d.suggestion.is_none()), "{whats:?}");
        assert!(whats.iter().all(|what| what.starts_with("couldn't open the board")), "{whats:?}");
        graph.get_mut(board).unwrap().disabled = true;
        runner.update(&ephys_rs::engine::Snapshot::new(1, graph));
        // Once the snapshot without it is the latest, and running: the one before (deciding) runs till then.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let without = |runner: &ephys_rs::engine::Runner<Nodes>| {
            runner.user_decisions().is_none() && runner.first_uses().iter().any(|first| first.version == 1)
        };
        let mut done = false;
        while !done && std::time::Instant::now() < until {
            done = without(&runner);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(done, "running without it, with nothing to decide");
    }

    /// The settings, running, with a board whose headstages differ from those set. See tests/snapshots/.
    #[test]
    fn settings_window() {
        let mut graph = Graph::<Nodes>::default();
        let node_type = NodeType::AcquisitionBoard;
        let board = graph.add(Nodes::create(node_type), [0.0, 0.0]);
        // As it's shown by default, outside tests.
        if let Some(Nodes::AcquisitionBoard(b)) = graph.get_mut(board).map(|e| &mut e.node) {
            b.source = Source::Fake { address: super::super::FAKE_ADDRESS.into() };
        }
        let info = BoardInfo {
            description: "Fake acquisition board (synthetic_streams/fake_acquisition_board.py)".into(),
            headstages: vec![Headstage { port: "A1".into(), channels: 64 }],
            adc_channels: 8,
            ttl_lines: 8,
        };
        let status = Status {
            state: State::Running,
            info: Some(info),
            samples: 912_000,
            mismatched: 912_000,
            rate: 30000.0,
            ..Default::default()
        };
        let display = BoardDisplay { status, pool: PoolStatus::default() };
        let shared = ephys_rs::engine::SharedByNode::default().with(board, Pool::holding(display));
        let mut harness = egui_kittest::Harness::builder().with_size([420.0, 560.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                graph.edit(
                    board,
                    &[],
                    &THEMES[0].colours,
                    Some(&shared),
                    None,
                    None,
                    true,
                    false,
                    None,
                    |node, context| node.on_gui_tick(Some(ui), context),
                );
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("board_settings");
    }
}
