//! The Acquisition Board node: samples from a board's headstages, and its auxiliary, analogue and digital
//! inputs, acquired on a thread of its own (see `capture`) and taken into each pass.

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use super::capture::{self, Capture, State, Status};
use super::{AUX_PER_HEADSTAGE, Bandwidth, Block, BoardInfo, Config, Headstage, SAMPLE_RATES, Source};
use crate::core::engine::{Category, Context, Node, NodeInfo, Output, Pass};
use crate::standard_components::ephys::{Continuous, Event, Events};
use crate::standard_components::widgets::{named_settings_grid, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind};

/// The board's analogue inputs.
pub const ADC_CHANNELS: u32 = 8;
/// The board's digital inputs.
pub const TTL_LINES: u32 = 8;

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
            headstages: vec![Headstage { port: 'A', channels: 32 }, Headstage { port: 'B', channels: 32 }],
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
    /// A block partly taken into the passes so far, and how much of it.
    pending: Option<(Block, usize)>,
    /// The digital inputs as of the last sample taken.
    ttl: u32,
    /// Samples skipped because the board's inputs aren't those expected.
    mismatched: u64,
    /// Digital input changes lost because a pass had no room for them.
    lost_events: u64,
}

/// How acquisition is going, for the node's window.
#[derive(Default)]
pub struct BoardDisplay {
    status: Status,
    mismatched: u64,
    lost_events: u64,
}

impl AcquisitionBoard {
    fn config(&self) -> Config {
        Config { sample_rate: self.sample_rate, bandwidth: self.bandwidth }
    }

    /// The outputs' indices: headstages and digital inputs, then auxiliary and analogue inputs if they're on.
    fn aux_output(&self) -> Option<usize> {
        self.aux.then_some(2)
    }

    fn adc_output(&self) -> Option<usize> {
        self.adc.then_some(2 + self.aux as usize)
    }

    /// Whether a block has the inputs expected.
    fn matches(&self, block: &Block) -> bool {
        block.channels(&block.ephys) == Headstage::total(&self.headstages) as usize
            && (!self.aux || block.channels(&block.aux) == (AUX_PER_HEADSTAGE as usize * self.headstages.len()))
            && (!self.adc || block.channels(&block.adc) == ADC_CHANNELS as usize)
    }
}

impl Node<Edges> for AcquisitionBoard {
    const INFO: NodeInfo = NodeInfo {
        name: "Acquisition Board",
        category: Category::Source,
        description: "Headstage channels, and auxiliary, analogue and digital inputs, from an acquisition board: \
                      the fake board in synthetic_streams/ for now.",
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
                stream: Some(DataStream::Events(Events { lines: TTL_LINES })),
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

    fn display(&self) -> Option<Box<dyn std::any::Any + Send>> {
        Some(Box::new(BoardDisplay::default()))
    }

    fn initial_state(&self, _inputs: &[Option<DataStream>]) -> Acquiring {
        Acquiring { capture: Some(Capture::start(self.source.clone(), self.config())), ..Default::default() }
    }

    /// Takes the samples that have arrived since the last pass, as many as there's room for (any more wait for
    /// the next), and makes events of the digital inputs' changes.
    fn process(&self, pass: &mut Pass<Edges>, acquiring: &mut Acquiring) {
        let Some(capture) = &acquiring.capture else { return };
        let (aux_output, adc_output) = (self.aux_output(), self.adc_output());
        let room = match pass.outputs.first() {
            Some(Buffer::Continuous(out)) => out.capacity,
            _ => return,
        };
        if let Some(Buffer::Events(events)) = pass.outputs.get_mut(1) {
            events.events.clear();
        }
        let continuous = [Some(0), aux_output, adc_output];
        let mut set_len = |len| {
            for output in continuous.into_iter().flatten() {
                if let Some(Buffer::Continuous(out)) = pass.outputs.get_mut(output) {
                    out.len = len;
                }
            }
        };
        set_len(room); // to fill, then down to what's filled
        let mut taken = 0;
        while taken < room {
            let (block, offset) = match acquiring.pending.take() {
                Some(pending) => pending,
                None => match capture.next() {
                    Some(block) => (block, 0),
                    None => break,
                },
            };
            if !self.matches(&block) {
                acquiring.mismatched += (block.samples - offset) as u64;
                capture.recycle(block);
                continue;
            }
            let n = (block.samples - offset).min(room - taken);
            let mut copy = |output: Option<usize>, values: &[f32]| {
                let Some(Buffer::Continuous(out)) = output.and_then(|o| pass.outputs.get_mut(o)) else { return };
                for channel in 0..out.channels {
                    let from = channel * block.samples + offset;
                    out.channel_mut(channel)[taken..taken + n].copy_from_slice(&values[from..from + n]);
                }
            };
            copy(Some(0), &block.ephys);
            copy(aux_output, &block.aux);
            copy(adc_output, &block.adc);
            if let Some(Buffer::Events(events)) = pass.outputs.get_mut(1) {
                for (i, &word) in block.ttl[offset..offset + n].iter().enumerate() {
                    let changed = (word ^ acquiring.ttl) & ((1 << TTL_LINES) - 1);
                    for line in (0..TTL_LINES).filter(|line| changed & (1 << line) != 0) {
                        if events.events.len() < events.events.capacity() {
                            let high = word & (1 << line) != 0;
                            events.events.push(Event { sample: (taken + i) as u32, line: line as u8, high });
                        } else {
                            acquiring.lost_events += 1;
                        }
                    }
                    acquiring.ttl = word;
                }
            }
            taken += n;
            if offset + n < block.samples {
                acquiring.pending = Some((block, offset + n));
            } else {
                capture.recycle(block);
            }
        }
        for output in continuous.into_iter().flatten() {
            if let Some(Buffer::Continuous(out)) = pass.outputs.get_mut(output) {
                out.len = taken;
            }
        }
        if let Some(display) = pass.display::<BoardDisplay>() {
            capture.status_into(&mut display.status);
            (display.mismatched, display.lost_events) = (acquiring.mismatched, acquiring.lost_events);
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        let theme = context.theme;
        settings_grid(ui, |ui| {
            setting(ui, "Board", |ui| source_ui(ui, &mut self.source));
            match &mut self.source {
                Source::Fake { address } => {
                    setting(ui, "Address", |ui| ui.text_edit_singleline(address));
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
            status.rate, status.samples, status.board_dropped, status.queue_dropped
        ));
        if display.mismatched > 0
            && let Some(info) = &status.info
        {
            let message = format!(
                "{} samples skipped: the board has headstages {}, not the {} set.",
                display.mismatched,
                Headstage::describe(&info.headstages),
                Headstage::describe(&self.headstages)
            );
            ui.label(RichText::new(message).color(theme.warning));
            if ui.button("Use the board's headstages").clicked() {
                self.headstages = info.headstages.clone();
            }
        }
        if display.lost_events > 0 {
            let message = format!("{} digital input changes lost (too many in a pass)", display.lost_events);
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
    };
    let choices = [Source::default()];
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
                    for channels in [16, 32, 64, 128] {
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
    let next_port = ('A'..='H').find(|port| headstages.iter().all(|h| h.port != *port));
    if let Some(port) = next_port
        && ui.small_button("Add headstage").clicked()
    {
        headstages.push(Headstage { port, channels: 32 });
        headstages.sort_by_key(|h| h.port);
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
            headstages: vec![Headstage { port: 'A', channels: 64 }],
            adc_channels: 8,
            ttl_lines: 8,
        };
        let status =
            Status { state: State::Running, info: Some(info), samples: 912_000, rate: 30000.0, ..Default::default() };
        let display = BoardDisplay { status, mismatched: 912_000, lost_events: 0 };
        let mut harness = egui_kittest::Harness::builder().with_size([420.0, 560.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                let display: &(dyn std::any::Any + Send) = &display;
                graph
                    .edit(board, &[], &THEMES[0].colours, Some(display), |node, context| node.settings_ui(ui, context));
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("board_settings");
    }
}
