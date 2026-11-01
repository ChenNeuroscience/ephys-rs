//! Write Raw: recording continuous data and digital inputs as they are, in Open Ephys's binary format, which its
//! analysis tools (and anything reading NumPy files) can load:
//!
//! ```text
//! name/Record Node 101/experiment1/recording1/
//!     structure.oebin                                   what's recorded, as JSON
//!     continuous/ephys-rs-100.continuous/
//!         continuous.dat                                16-bit samples, channel by channel within each sample
//!         sample_numbers.npy, timestamps.npy            each sample's number, and time in seconds
//!     events/ephys-rs-100.events/TTL/
//!         states.npy                                    each change: +line going high, -line going low (from 1)
//!         sample_numbers.npy, timestamps.npy, full_words.npy
//! ```
//!
//! Samples are numbered, and timed, from the start of recording.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use chrono::Local;
use eframe::egui;
use serde::{Deserialize, Serialize};

use super::npy::Npy;
use super::{Files, Status, Writing, path_in, session_name, status_ui};
use crate::core::engine::{
    Category, Context, CopyTo, EdgeSet, Input, Node, NodeInfo, NodeThread, Output, Pass, ThreadContext,
};
use crate::core::exchange::Latest;
use crate::standard_components::ephys::board::EPHYS_MICROVOLTS_PER_BIT;
use crate::standard_components::ephys::{Continuous, Event, Events};
use crate::standard_components::widgets::{require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind};

const CONTINUOUS_FOLDER: &str = "ephys-rs-100.continuous";
const EVENTS_FOLDER: &str = "ephys-rs-100.events";

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WriteRaw {
    pub directory: String,
    pub prefix: String,
}

impl Default for WriteRaw {
    fn default() -> Self {
        WriteRaw { directory: "~/recordings".into(), prefix: "session".into() }
    }
}

/// While running: the inputs' streams as the run started, the writer's thread while recording, and its status.
#[derive(Default)]
pub struct Recorder {
    continuous: Option<Continuous>,
    events: Option<Events>,
    writing: Option<Writing<Chunk>>,
    shown: Latest<Status>,
}

/// A pass's data, copied for the writer's thread.
#[derive(Default)]
pub struct Chunk {
    /// Channel after channel, `len` samples each.
    continuous: Vec<f32>,
    len: usize,
    events: Vec<Event>,
    event_pass_samples: usize,
}

impl Node<Edges> for WriteRaw {
    type State = Recorder;

    const INFO: NodeInfo = NodeInfo {
        name: "Write Raw",
        category: Category::Recording,
        description: "Records continuous data and digital inputs as they are, in Open Ephys's binary format.",
    };

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![Input { name: "continuous", kind: Kind::Continuous }, Input { name: "events", kind: Kind::Events }]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        require_input(inputs, "something to record")
    }

    fn copies(&self) -> Option<CopyTo> {
        Some(CopyTo::Thread)
    }

    fn threads(&self) -> Vec<NodeThread> {
        vec![NodeThread { name: "Raw writer", link: "chunk queue" }]
    }

    fn thread_ui(&self, _thread: usize, ui: &mut egui::Ui, context: &ThreadContext) {
        status_ui(ui, context.display::<Status>(), context.theme);
    }

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Recorder {
        let continuous = match inputs[0] {
            Some(DataStream::Continuous(c)) => Some(c),
            _ => None,
        };
        let events = match inputs[1] {
            Some(DataStream::Events(e)) => Some(e),
            _ => None,
        };
        Recorder { continuous, events, ..Default::default() }
    }

    /// Opens the files, and starts the thread to write them.
    fn prepare_to_record(
        &self,
        inputs: &[Option<DataStream>],
        _controls: &[Option<crate::Setting>],
    ) -> Option<Box<dyn std::any::Any + Send>> {
        let Recorder { continuous, events, .. } = self.initial_state(inputs);
        let folder = self.folder();
        let files = RawFiles::open(&folder, continuous, events);
        let writing: Writing<Chunk> =
            Writing::new("Raw writer", folder.display().to_string(), files.map(|f| Box::new(f) as Box<_>));
        Some(Box::new(writing))
    }

    /// While recording, copies the pass's data for the writer's thread.
    fn process(&self, pass: &mut Pass<Edges>, recorder: &mut Recorder) {
        match (pass.recording, &recorder.writing) {
            (true, None) => {
                recorder.writing = pass.take_prepared::<Writing<Chunk>>().or_else(|| {
                    // Not prepared (the graph changed while recording, say): the thread opens the files.
                    let folder = self.folder();
                    let (continuous, events) = (recorder.continuous, recorder.events);
                    let shown = folder.display().to_string();
                    Some(Writing::start("Raw writer", shown, move || {
                        RawFiles::open(&folder, continuous, events).map(|f| Box::new(f) as Box<_>)
                    }))
                });
            }
            (false, Some(_)) => recorder.writing = None, // finishes the files
            _ => {}
        }
        if let Some(writing) = &recorder.writing {
            let mut chunk = writing.chunk();
            chunk.continuous.clear();
            chunk.len = 0;
            if let Some(Some(Buffer::Continuous(continuous))) = pass.inputs.first() {
                for channel in 0..continuous.channels {
                    chunk.continuous.extend_from_slice(&continuous.channel(channel)[..continuous.len]);
                }
                chunk.len = continuous.len;
            }
            chunk.events.clear();
            chunk.event_pass_samples = 0;
            if let Some(Some(Buffer::Events(events))) = pass.inputs.get(1) {
                chunk.events.extend_from_slice(&events.events);
                chunk.event_pass_samples = events.pass_samples;
            }
            writing.send(chunk);
        }
        let writing = &recorder.writing;
        recorder.shown.send(pass, |status| match writing {
            Some(writing) => writing.status_into(status),
            None => *status = Status::default(),
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            setting(ui, "Directory", |ui| ui.text_edit_singleline(&mut self.directory));
            setting(ui, "Folder prefix", |ui| ui.text_edit_singleline(&mut self.prefix));
        });
        ui.add_space(4.0);
        let recording: Vec<String> = self
            .inputs()
            .iter()
            .zip(context.inputs)
            .filter_map(|(input, stream)| stream.map(|s| format!("{}: {}", input.name, s.summary())))
            .collect();
        if recording.is_empty() {
            ui.weak("Nothing connected to record.");
        } else {
            ui.label(format!("Records {}.", recording.join(", ")));
        }
        ui.separator();
        status_ui(ui, context.display::<Status>(), context.theme);
    }
}

impl WriteRaw {
    /// A new recording's folder.
    fn folder(&self) -> std::path::PathBuf {
        path_in(&self.directory, &session_name(&self.prefix, &Local::now()))
    }
}

struct Samples {
    stream: Continuous,
    data: BufWriter<File>,
    numbers: Npy,
    times: Npy,
    count: u64,
    /// Each sample's values, as 16-bit integers.
    row: Vec<u8>,
}

struct Ttl {
    stream: Events,
    states: Npy,
    numbers: Npy,
    times: Npy,
    words: Npy,
    /// Samples of the events' passes so far, and the lines' states.
    samples: u64,
    word: u64,
}

/// A recording's files, written on the writer's thread.
struct RawFiles {
    continuous: Option<Samples>,
    events: Option<Ttl>,
}

impl RawFiles {
    fn open(folder: &Path, continuous: Option<Continuous>, events: Option<Events>) -> io::Result<Self> {
        let recording = folder.join("Record Node 101").join("experiment1").join("recording1");
        std::fs::create_dir_all(&recording)?;
        std::fs::write(recording.join("structure.oebin"), structure(continuous, events))?;
        let continuous = match continuous {
            Some(stream) => {
                let dir = recording.join("continuous").join(CONTINUOUS_FOLDER);
                std::fs::create_dir_all(&dir)?;
                Some(Samples {
                    stream,
                    data: BufWriter::new(File::create(dir.join("continuous.dat"))?),
                    numbers: Npy::create(&dir.join("sample_numbers.npy"), "<i8")?,
                    times: Npy::create(&dir.join("timestamps.npy"), "<f8")?,
                    count: 0,
                    row: Vec::new(),
                })
            }
            None => None,
        };
        let events = match events {
            Some(stream) => {
                let dir = recording.join("events").join(EVENTS_FOLDER).join("TTL");
                std::fs::create_dir_all(&dir)?;
                Some(Ttl {
                    stream,
                    states: Npy::create(&dir.join("states.npy"), "<i2")?,
                    numbers: Npy::create(&dir.join("sample_numbers.npy"), "<i8")?,
                    times: Npy::create(&dir.join("timestamps.npy"), "<f8")?,
                    words: Npy::create(&dir.join("full_words.npy"), "<u8")?,
                    samples: 0,
                    word: 0,
                })
            }
            None => None,
        };
        log::info!("Recording raw data to {}", folder.display());
        Ok(RawFiles { continuous, events })
    }
}

/// `structure.oebin`: what's recorded, and how to read it.
fn structure(continuous: Option<Continuous>, events: Option<Events>) -> String {
    let continuous: Vec<serde_json::Value> = continuous
        .map(|c| {
            let channels: Vec<serde_json::Value> = (0..c.channels)
                .map(|i| {
                    serde_json::json!({
                        "channel_name": format!("CH{}", i + 1),
                        "description": "",
                        "identifier": "",
                        "history": "",
                        "bit_volts": EPHYS_MICROVOLTS_PER_BIT,
                        "units": "uV",
                    })
                })
                .collect();
            serde_json::json!({
                "folder_name": format!("{CONTINUOUS_FOLDER}/"),
                "sample_rate": c.sample_rate,
                "source_processor_name": "ephys-rs",
                "source_processor_id": 100,
                "stream_name": "continuous",
                "recorded_processor": "ephys-rs",
                "recorded_processor_id": 100,
                "num_channels": c.channels,
                "channels": channels,
            })
        })
        .into_iter()
        .collect();
    let events: Vec<serde_json::Value> = events
        .map(|e| {
            serde_json::json!({
                "folder_name": format!("{EVENTS_FOLDER}/TTL/"),
                "channel_name": "TTL",
                "description": "Digital inputs",
                "identifier": "",
                "sample_rate": e.sample_rate,
                "type": "int16",
                "num_channels": e.lines,
                "source_processor": "ephys-rs",
                "stream_name": "events",
                "initial_state": 0,
            })
        })
        .into_iter()
        .collect();
    let structure = serde_json::json!({
        "GUI version": "0.6.7",
        "continuous": continuous,
        "events": events,
        "spikes": [],
    });
    serde_json::to_string_pretty(&structure).unwrap_or_default()
}

impl Files<Chunk> for RawFiles {
    fn write(&mut self, chunk: &Chunk) -> io::Result<u64> {
        let mut bytes = 0;
        if let Some(samples) = &mut self.continuous
            && chunk.len > 0
        {
            let (channels, len, rate) =
                (samples.stream.channels as usize, chunk.len, samples.stream.sample_rate as f64);
            for t in 0..len {
                samples.row.clear();
                for channel in 0..channels {
                    let value = chunk.continuous.get(channel * len + t).copied().unwrap_or(0.0);
                    let step = (value / EPHYS_MICROVOLTS_PER_BIT).round().clamp(i16::MIN as f32, i16::MAX as f32);
                    samples.row.extend_from_slice(&(step as i16).to_le_bytes());
                }
                samples.data.write_all(&samples.row)?;
                let number = samples.count + t as u64;
                samples.numbers.write_i64(number as i64)?;
                samples.times.write_f64(number as f64 / rate)?;
                bytes += samples.row.len() as u64 + 16;
            }
            samples.count += len as u64;
        }
        if let Some(ttl) = &mut self.events {
            let rate = ttl.stream.sample_rate as f64;
            for event in &chunk.events {
                let line = event.line as u64;
                if event.high {
                    ttl.word |= 1 << line;
                } else {
                    ttl.word &= !(1 << line);
                }
                let state = (line as i16 + 1) * if event.high { 1 } else { -1 };
                let number = ttl.samples + event.sample as u64;
                ttl.states.write(&state.to_le_bytes(), 1)?;
                ttl.numbers.write_i64(number as i64)?;
                ttl.times.write_f64(number as f64 / rate)?;
                ttl.words.write(&ttl.word.to_le_bytes(), 1)?;
                bytes += 26;
            }
            ttl.samples += chunk.event_pass_samples as u64;
        }
        Ok(bytes)
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        if let Some(mut samples) = self.continuous {
            samples.data.flush()?;
            samples.numbers.finish()?;
            samples.times.finish()?;
        }
        if let Some(ttl) = self.events {
            ttl.states.finish()?;
            ttl.numbers.finish()?;
            ttl.times.finish()?;
            ttl.words.finish()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_samples_interleaved_and_events() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-raw-{}", std::process::id()));
        let continuous = Continuous { channels: 2, sample_rate: 1000.0 };
        let events = Events { lines: 8, sample_rate: 1000.0 };
        let mut files = Box::new(RawFiles::open(&dir, Some(continuous), Some(events)).unwrap());
        let chunk = Chunk {
            // Two channels of three samples: 1, 2, 3 and 10, 20, 30 steps.
            continuous: [1.0, 2.0, 3.0, 10.0, 20.0, 30.0].map(|s| s * EPHYS_MICROVOLTS_PER_BIT).to_vec(),
            len: 3,
            events: vec![Event { sample: 2, line: 1, high: true }],
            event_pass_samples: 3,
        };
        files.write(&chunk).unwrap();
        files.write(&chunk).unwrap();
        files.finish().unwrap();
        let recording = dir.join("Record Node 101/experiment1/recording1");
        let data = std::fs::read(recording.join("continuous").join(CONTINUOUS_FOLDER).join("continuous.dat")).unwrap();
        let values: Vec<i16> = data.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
        assert_eq!(&values[..6], &[1, 10, 2, 20, 3, 30]);
        assert_eq!(values.len(), 12);
        let numbers =
            std::fs::read(recording.join("events").join(EVENTS_FOLDER).join("TTL/sample_numbers.npy")).unwrap();
        let second = i64::from_le_bytes(numbers[128 + 8..128 + 16].try_into().unwrap());
        assert_eq!(second, 5, "the second pass's event, 3 samples on");
        let oebin: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(recording.join("structure.oebin")).unwrap()).unwrap();
        assert_eq!(oebin["continuous"][0]["num_channels"], 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
