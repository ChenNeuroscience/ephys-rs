//! The Acquisition Board node: samples from a board's headstages, and its auxiliary, analogue and digital
//! inputs, acquired on a thread of its own (see `capture`) into a pool of buffers, which each pass swaps for its
//! own.

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use super::capture::{self, Capture, Shape, State, Status};
use super::{AUX_PER_HEADSTAGE, Bandwidth, BoardInfo, Config, Headstage, OpalKellyKind, PORTS, SAMPLE_RATES, Source};
use crate::core::engine::{
    Category, Context, Flow, Node, NodeInfo, NodeThread, Output, Pass, PoolStatus, ThreadContext,
};
use crate::core::exchange::Latest;
use crate::standard_components::ephys::{Continuous, Events};
use crate::standard_components::widgets::{named_settings_grid, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind};

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
    shown: Latest<BoardDisplay>,
}

impl crate::core::engine::PassState for Acquiring {
    const WHY: &'static str = "The board's capture thread and its pool of buffers: each pass takes the samples \
                               that have come since the last, in order.";
}

/// How acquisition is going, for the node's window.
#[derive(Default)]
pub struct BoardDisplay {
    status: Status,
    pool: PoolStatus,
}

impl AcquisitionBoard {
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

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let rate = self.sample_rate as f32;
        let continuous = |channels| Some(DataStream::Continuous(Continuous { channels, sample_rate: rate }));
        let mut outputs = vec![
            Output {
                name: "headstages",
                kind: Kind::Continuous,
                stream: continuous(Headstage::total(&self.headstages)),
            },
            Output {
                name: "digital",
                kind: Kind::Events,
                stream: Some(DataStream::Events(Events { lines: self.source.ttl_lines(), sample_rate: rate })),
            },
        ];
        if self.aux {
            let channels = AUX_PER_HEADSTAGE * self.headstages.len() as u32;
            outputs.push(Output { name: "auxiliary", kind: Kind::Continuous, stream: continuous(channels) });
        }
        if self.adc {
            outputs.push(Output { name: "analogue", kind: Kind::Continuous, stream: continuous(ADC_CHANNELS) });
        }
        outputs
    }

    fn warnings(&self, _inputs: &[Option<DataStream>]) -> Vec<String> {
        if self.headstages.is_empty() {
            vec!["No headstages: add some, or detect the board's".into()]
        } else {
            Vec::new()
        }
    }

    fn threads(&self) -> Vec<NodeThread> {
        vec![NodeThread {
            name: "Board capture",
            link: "buffer pool",
            flow: Flow::ToPasses,
            why: "On a thread of its own so samples are taken from the board as they come (before its buffer \
                  overflows), whatever the passes are doing.",
            swaps: true,
        }]
    }

    fn thread_ui(&self, _thread: usize, ui: &mut egui::Ui, context: &ThreadContext) {
        let small = |text: String| RichText::new(text).size(11.0);
        let Some(display) = context.display::<BoardDisplay>() else {
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

    fn thread_pool(&self, _thread: usize, context: &ThreadContext) -> Option<PoolStatus> {
        context.display::<BoardDisplay>().map(|d| d.pool)
    }

    fn initial_state(&self, _inputs: &[Option<DataStream>]) -> Acquiring {
        let rate = self.sample_rate as f32;
        let continuous = |channels| Continuous { channels, sample_rate: rate };
        let shape = Shape {
            ephys: continuous(Headstage::total(&self.headstages)),
            aux: continuous(AUX_PER_HEADSTAGE * self.headstages.len() as u32),
            adc: continuous(ADC_CHANNELS),
            events: Events { lines: self.source.ttl_lines(), sample_rate: rate },
        };
        Acquiring { capture: Some(Capture::start(self.source.clone(), self.config(), shape)), ..Default::default() }
    }

    /// Takes the samples that have arrived since the last pass, swapping the buffers they're in for the pass's
    /// own (which go back to the capture thread's pool to fill), so nothing's copied.
    fn process(&self, pass: &mut Pass<Edges>, acquiring: &mut Acquiring) {
        let Some(capture) = &acquiring.capture else { return };
        // The headstages', auxiliary and analogue inputs' outputs, if they're on.
        let outputs = [Some(0), self.aux_output(), self.adc_output()];
        match capture.pool().take(|samples| samples.len() == 0) {
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
        acquiring.shown.send(pass, |display| {
            capture.status_into(&mut display.status);
            display.pool = capture.pool().status();
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let theme = context.theme;
        settings_grid(ui, |ui| {
            setting(ui, "Board", |ui| source_ui(ui, &mut self.source));
            match &mut self.source {
                Source::Fake { address } => {
                    setting(ui, "Address", |ui| ui.text_edit_singleline(address));
                }
                Source::OpalKelly { board, bitfile, library } => {
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
                    let path = |ui: &mut egui::Ui, value: &mut String| {
                        ui.add(egui::TextEdit::singleline(value).hint_text("found automatically"))
                    };
                    setting(ui, "Bitfile", |ui| path(ui, bitfile));
                    setting(ui, "Library", |ui| path(ui, library));
                }
                Source::Oni { library } => {
                    setting(ui, "Library", |ui| {
                        ui.add(egui::TextEdit::singleline(library).hint_text("found automatically"))
                    });
                }
            }
            setting(ui, "Sample rate", |ui| {
                egui::ComboBox::from_id_salt("rate")
                    .selected_text(format!("{} Hz", self.sample_rate))
                    .show_ui(ui, |ui| {
                        for rate in SAMPLE_RATES {
                            ui.selectable_value(&mut self.sample_rate, rate, format!("{rate} Hz"));
                        }
                    })
                    .response
            });
        });

        ui.add_space(6.0);
        ui.label(RichText::new("Headstages").strong());
        headstages_ui(ui, &mut self.headstages);
        detect_ui(ui, self, theme);

        ui.add_space(6.0);
        ui.checkbox(&mut self.aux, format!("Auxiliary inputs ({AUX_PER_HEADSTAGE} per headstage)"));
        ui.checkbox(&mut self.adc, format!("Analogue inputs ({ADC_CHANNELS})"));

        ui.add_space(6.0);
        ui.label(RichText::new("Bandwidth").strong());
        named_settings_grid(ui, "bandwidth", |ui| {
            let upper = self.bandwidth.upper_hz;
            setting(ui, "Low cut", |ui| ui.add(hz(&mut self.bandwidth.lower_hz, 0.1..=upper)));
            setting(ui, "High cut", |ui| ui.add(hz(&mut self.bandwidth.upper_hz, 100.0..=20000.0)));
            setting(ui, "DSP high-pass", |ui| {
                ui.horizontal(|ui| {
                    let mut on = self.bandwidth.dsp_hz.is_some();
                    ui.checkbox(&mut on, "on");
                    self.bandwidth.dsp_hz = on.then(|| {
                        let mut cutoff = self.bandwidth.dsp_hz.unwrap_or(0.5);
                        ui.add(hz(&mut cutoff, 0.01..=1000.0));
                        cutoff
                    });
                })
                .response
            });
        });

        ui.separator();
        let Some(display) = context.display::<BoardDisplay>() else {
            ui.weak("Press Play to start acquiring.");
            return;
        };
        let status = &display.status;
        match &status.state {
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
        ui.weak(format!(
            "{:.0} samples/s · {} samples · {} dropped by the board · {} not taken in time",
            status.rate, status.samples, status.board_dropped, status.not_taken
        ));
        if status.mismatched > 0
            && let Some(info) = &status.info
        {
            let message = format!(
                "{} samples skipped: the board has headstages {}, not the {} set.",
                status.mismatched,
                Headstage::describe(&info.headstages),
                Headstage::describe(&self.headstages)
            );
            ui.label(RichText::new(message).color(theme.warning));
            if ui.button("Use the board's headstages").clicked() {
                self.headstages = info.headstages.clone();
            }
        }
        if status.lost_events > 0 {
            let message = format!("{} digital input changes lost (too many in a pass)", status.lost_events);
            ui.label(RichText::new(message).color(theme.warning));
        }
        for warning in &status.warnings {
            ui.label(RichText::new(warning).small().color(theme.warning));
        }
    }
}

fn source_ui(ui: &mut egui::Ui, source: &mut Source) -> egui::Response {
    let name = |source: &Source| match source {
        Source::Fake { .. } => "Fake board (synthetic_streams/)",
        Source::OpalKelly { .. } => "Open Ephys gen 1/2, or Intan USB",
        Source::Oni { .. } => "Open Ephys gen 3",
    };
    let choices = [
        Source::default(),
        Source::OpalKelly { board: OpalKellyKind::OpenEphys, bitfile: String::new(), library: String::new() },
        Source::Oni { library: String::new() },
    ];
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
            if ui.small_button("×").on_hover_text("Remove headstage").clicked() {
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
fn detect_ui(ui: &mut egui::Ui, board: &mut AcquisitionBoard, theme: &crate::core::theme::Colours) {
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
    use crate::Nodes;
    use crate::core::engine::{Graph, NodeSet};
    use crate::standard_components::themes::THEMES;

    /// The settings, running, with a board whose headstages differ from those set. See tests/snapshots/.
    #[test]
    fn settings_window() {
        let mut graph = Graph::<Nodes>::default();
        let index = Nodes::TYPES.iter().position(|t| t.name == "Acquisition Board").unwrap();
        let board = graph.add(Nodes::create(index), [0.0, 0.0]);
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
        let mut harness = egui_kittest::Harness::builder().with_size([420.0, 560.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                let display: &(dyn std::any::Any + Send) = &display;
                graph.edit(board, &[], &THEMES[0].colours, Some(display), true, false, None, |node, context| {
                    node.settings_ui(ui, context)
                });
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("board_settings");
    }
}
