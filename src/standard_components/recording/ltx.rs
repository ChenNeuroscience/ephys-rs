//! Write LTX: recording in the LTX format, as the Open Ephys LTX plugin writes it, for legacy tetrode analysis
//! code (and Waveform GUI). For a recording called `name` (the prefix and when it started):
//!
//! - `name.set`: headers only.
//! - `name.1`, `name.2`...: each tetrode's spikes, each as 4 × (a 4-byte timestamp, then 50 one-byte samples).
//! - `name.egf`, `name.egf2`...: each continuous channel at 5 kHz, a byte a sample.
//! - `name.pos`: LED positions, a 4-byte timestamp then 8 two-byte values (x1, y1, x2, y2, and the LEDs' sizes).
//! - `name.ttl`: digital input changes, as text.
//! - `name.pos.ttl`: the timing LED's changes, likewise, from its `pos ttl` input, if there's a timing LED. They're timed
//!   by the camera's clock, as positions are, counting from the same first position.
//! - `name.txt`: log lines, with no headers: each as `<seconds> <line>`, its time counting from the same first position.
//!
//! Each file but `name.txt` is `key value` header lines, then the data between `data_start` and `data_end`; counts and the
//! duration are filled in at the end. Timestamps count 96 kHz ticks from the start of recording, and multi-byte
//! values are big-endian. Spikes' samples are -128 to 127: their waveforms should come as int8 values (as floats),
//! multiplied into them by a Spike Gain, whose gains (connected, read-only, to `gains`) are recorded in the `.set`
//! file, with the full scale, as Axona's are (`gain_ch_0`..., `ADC_fullscale_mv`). The EEG (its `eeg` input, kept at
//! 5 kHz) should come as int8 values too, multiplied into them by a Continuous Gain, whose gains (connected, read-only,
//! to `eeg gains`) are recorded likewise, as `EEG_gain_1`... in the `.set` file, and `gain` in each `.egf` file. Which
//! channel each EEG channel is (from the Channel Selector choosing them, connected to `eeg selection`) is recorded as
//! `EEG_ch_1`... in the `.set` file, and `chan_name` in each `.egf` file.
//!
//! The positions' header records the position detector's settings (connected, read-only, to `detection`): the
//! crop, whose bounding box positions are measured from (scaled down to fit 0–1022 if need be; 1023 means no LED),
//! and the calibration. And it records anything in its `custom` settings as headers of
//! their own, as they are (whatever a reader makes of a key with a space in it, or one that's the same as another header),
//! except that line breaks are written as spaces, as a header's a line.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use eframe::egui;
use serde::{Deserialize, Serialize};

use super::{
    Files, Location, OnFinished, Status, Writing, after_recording, location_summary, location_ui, recording_name,
    status_ui,
};
use crate::core::engine::{
    Category, Context, Control, DataStreamType, EdgeSet, Flow, Input, Node, NodeInfo, NodeThread, Output,
    PASS_DURATION, Pass, PoolStatus, ThreadContext,
};
use crate::standard_components::ephys::reference::parse_channels;
use crate::standard_components::ephys::{ChannelSelection, Continuous, Event, Events};
use crate::standard_components::logs::{LINES_PER_PASS, LogLines};
use crate::standard_components::pool::{Pool, SharedPool};
use crate::standard_components::spikes::{Gains, Spikes};
use crate::standard_components::tracking::{Crop, Detection, PositionSample, Positions};
use crate::standard_components::widgets::{require_input, setting, settings_grid};
use crate::{Buffer, DataStream, Edges, Kind, Setting, SettingKind};

/// Ticks a second, for timestamps.
/// Its control for what runs after a recording.
const AFTER_RECORDING: usize = 4;

const TIMEBASE: f64 = 96000.0;
/// The continuous data's rate, once downsampled.
const EGF_RATE: u32 = 5000;
/// Samples stored for each channel of a spike.
const SPIKE_SAMPLES: usize = 50;
const SPIKE_CHANNELS: usize = 4;
/// What a position that wasn't found is stored as.
const MISSING: u16 = 1023;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WriteLtx {
    /// Where it writes (each recording's files in it).
    pub location: Location,
    pub prefix: String,
}

impl Default for WriteLtx {
    fn default() -> Self {
        WriteLtx { location: Location::Base, prefix: "session".into() }
    }
}

/// The inputs' data streams, as the run started.
#[derive(Clone, Copy, Default)]
struct Streams {
    spikes: Option<Spikes>,
    continuous: Option<Continuous>,
    positions: Option<Positions>,
    events: Option<Events>,
    position_events: Option<Events>,
    log: Option<LogLines>,
}

/// While running: the writer's thread while recording, and its status for the window.
#[derive(Default)]
pub struct Recorder {
    streams: Streams,
    writing: Option<Writing<Chunk>>,
    /// Continuous samples so far while recording: the EEG keeps every so many (see `egf_step`), carrying on from
    /// pass to pass.
    continuous_samples: u64,
    shown: SharedPool<Status>,
}

impl crate::core::engine::PassState for Recorder {
    const WHY: &'static str = "The writer's thread while recording, which each pass's copy goes to, in order, \
                               so the files are written in order.";
}

/// A pass's data, copied for the writer's thread (keeping only what's written).
#[derive(Default)]
pub struct Chunk {
    /// Each spike's group and peak (a sample of its pass), and their waveforms (as in `SpikesBuffer`).
    spikes: Vec<(u32, i32)>,
    waveforms: Vec<f32>,
    spike_pass_samples: usize,
    /// The continuous data the EEG keeps (at 5 kHz), channel after channel, `continuous_len` samples each.
    continuous: Vec<f32>,
    continuous_len: usize,
    positions: Vec<PositionSample>,
    events: Vec<Event>,
    event_pass_samples: usize,
    position_events: Vec<Event>,
    position_event_pass_samples: usize,
    /// The log lines, one after another, and each one's time and length.
    log: Vec<u8>,
    log_lines: Vec<(f64, u32)>,
}

impl Chunk {
    /// Empty, with room for a pass of `streams`, so it doesn't grow as it's filled.
    fn new(streams: Streams) -> Self {
        let spikes = streams.spikes.map(|s| s.buffer(PASS_DURATION));
        let eeg = streams.continuous.map_or(0, |c| {
            let kept = egf_step(c.sample_rate).map_or(0, |step| c.buffer(PASS_DURATION).capacity.div_ceil(step));
            c.channels as usize * kept
        });
        Chunk {
            spikes: Vec::with_capacity(spikes.as_ref().map_or(0, |s| s.spikes.capacity())),
            waveforms: Vec::with_capacity(spikes.as_ref().map_or(0, |s| s.waveforms.capacity())),
            continuous: Vec::with_capacity(eeg),
            positions: Vec::with_capacity(streams.positions.map_or(0, |p| p.buffer(PASS_DURATION).samples.capacity())),
            events: Vec::with_capacity(streams.events.map_or(0, |e| e.buffer(PASS_DURATION).events.capacity())),
            position_events: Vec::with_capacity(
                streams.position_events.map_or(0, |e| e.buffer(PASS_DURATION).events.capacity()),
            ),
            log: Vec::with_capacity(streams.log.map_or(0, |l| l.width as usize * LINES_PER_PASS)),
            log_lines: Vec::with_capacity(streams.log.map_or(0, |_| LINES_PER_PASS)),
            ..Default::default()
        }
    }

    /// The memory one takes.
    fn bytes(&self) -> usize {
        self.spikes.capacity() * size_of::<(u32, i32)>()
            + (self.waveforms.capacity() + self.continuous.capacity()) * size_of::<f32>()
            + self.positions.capacity() * size_of::<PositionSample>()
            + (self.events.capacity() + self.position_events.capacity()) * size_of::<Event>()
            + self.log.capacity()
            + self.log_lines.capacity() * size_of::<(f64, u32)>()
    }

    /// The writer's thread, with a pool of chunks for `streams`, writing to `files`.
    fn writing(
        base: &Path,
        streams: Streams,
        files: io::Result<LtxFiles>,
        on_finished: Option<OnFinished>,
    ) -> Writing<Chunk> {
        let (shown, bytes) = (base.display().to_string(), Chunk::new(streams).bytes());
        let make = move || Chunk::new(streams);
        Writing::new("LTX writer", shown, files.map(|f| Box::new(f) as Box<_>), make, bytes, on_finished)
    }
}

impl Node<Edges> for WriteLtx {
    type State = Recorder;

    const INFO: NodeInfo = NodeInfo {
        name: "Write LTX",
        category: Category::Recording,
        description: "Records tetrode spikes, continuous data (at 5 kHz), positions, digital inputs, a timing LED's \
                      changes and log lines in the LTX format, for legacy tetrode analysis code.",
    };

    fn inputs(&self) -> Vec<Input<Edges>> {
        vec![
            Input { name: "spikes", kind: Kind::Spikes },
            Input { name: "eeg", kind: Kind::Continuous },
            Input { name: "positions", kind: Kind::Positions },
            Input { name: "events", kind: Kind::Events },
            Input { name: "pos ttl", kind: Kind::Events },
            Input { name: "log", kind: Kind::LogLines },
        ]
    }

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn controls(&self) -> Vec<Control<Edges>> {
        vec![
            Control { name: "detection", kind: SettingKind::Detection, read_only: true, on_right: false },
            Control { name: "gains", kind: SettingKind::Gains, read_only: true, on_right: false },
            Control { name: "eeg selection", kind: SettingKind::ChannelSelection, read_only: true, on_right: false },
            Control { name: "eeg gains", kind: SettingKind::Gains, read_only: true, on_right: false },
            Control { name: "after recording", kind: SettingKind::Command, read_only: true, on_right: true },
        ]
    }

    fn warnings(&self, inputs: &[Option<DataStream>]) -> Vec<String> {
        let mut warnings = require_input(inputs, "something to record");
        if let Some(DataStream::Spikes(spikes)) = inputs[0]
            && spikes.group_size != SPIKE_CHANNELS as u32
        {
            warnings.push(format!("LTX spikes are in tetrodes: groups of 4, not {}", spikes.group_size));
        }
        if let Some(DataStream::Continuous(continuous)) = inputs[1]
            && egf_step(continuous.sample_rate).is_none()
        {
            warnings.push(format!("LTX continuous data is {EGF_RATE} Hz: the input's rate must be a multiple"));
        }
        if inputs[4].is_some() && inputs[2].is_none() {
            warnings.push("Connect the positions too, so the pos ttl's times can be compared with them".into());
        }
        warnings
    }

    fn threads(&self) -> Vec<NodeThread> {
        vec![NodeThread {
            name: "LTX writer",
            link: "buffer pool",
            flow: Flow::FromPasses,
            why: super::WHY_A_THREAD,
            swaps: false,
        }]
    }

    fn thread_pool(&self, _thread: usize, context: &ThreadContext) -> Option<PoolStatus> {
        context.shared::<Pool<Status>>().and_then(|l| l.newest().map(|s| s.pool))
    }

    fn thread_ui(&self, _thread: usize, ui: &mut egui::Ui, context: &ThreadContext) {
        let pool = context.shared::<Pool<Status>>();
        status_ui(ui, pool.as_deref().and_then(Pool::newest).as_deref(), true, context.theme);
    }

    fn initial_state(&self, inputs: &[Option<DataStream>]) -> Recorder {
        Recorder { streams: Streams::of(inputs), ..Default::default() }
    }

    /// Opens the files, and starts the thread to write them.
    fn prepare_to_record(
        &self,
        inputs: &[Option<DataStream>],
        controls: &[Option<Setting>],
        base_directory: Option<&crate::core::base_directory::Place>,
        triggers: &crate::core::triggers::ControlTriggers,
    ) -> Option<Box<dyn std::any::Any + Send>> {
        let (start, base) = self.base(base_directory);
        let (streams, recorded) = (Streams::of(inputs), Recorded::of(controls));
        let files = LtxFiles::open(&base, start, streams, recorded);
        Some(Box::new(Chunk::writing(&base, streams, files, after_recording(triggers, AFTER_RECORDING))))
    }

    /// While recording, copies the pass's data for the writer's thread.
    fn process(&self, pass: &mut Pass<Edges>, recorder: &mut Recorder) {
        match (pass.recording, &recorder.writing) {
            (true, None) => {
                recorder.continuous_samples = 0;
                let writing = pass.take_prepared::<Writing<Chunk>>();
                recorder.writing = Some(writing.unwrap_or_else(|| Writing::failed("it wasn't prepared to record")));
            }
            (false, Some(_)) => recorder.writing = None, // finishes the files
            _ => {}
        }
        if let Some(writing) = &recorder.writing
            && let Some(mut chunk) = writing.chunk()
        {
            let step = recorder.streams.continuous.and_then(|c| egf_step(c.sample_rate));
            copy(pass, &mut chunk, step, &mut recorder.continuous_samples);
            writing.send(chunk);
        }
        let writing = &recorder.writing;
        recorder.shown.send_newest(pass, |status| match writing {
            Some(writing) => writing.status_into(status),
            None => *status = Status::default(),
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        settings_grid(ui, |ui| {
            location_ui(ui, &mut self.location, context.base_directory);
            setting(ui, "File prefix", |ui| ui.text_edit_singleline(&mut self.prefix));
        });
        location_summary(ui, &self.location, context.base_directory, &self.prefix, "files");
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
        if context.controls[0].is_none() && context.inputs[2].is_some() {
            ui.weak("Connect the detection to record the crop and calibration with the positions.");
        }
        if context.controls[1].is_none() && context.inputs[0].is_some() {
            ui.weak("Connect the Spike Gain's gains, to record them with the spikes.");
        }
        if (context.controls[2].is_none() || context.controls[3].is_none()) && context.inputs[1].is_some() {
            ui.weak("Connect the Channel Selector's selection and the Continuous Gain's gains, to record them with the EEG.");
        }
        ui.separator();
        let pool = context.shared::<Pool<Status>>();
        status_ui(ui, pool.as_deref().and_then(Pool::newest).as_deref(), context.running, context.theme);
    }
}

impl WriteLtx {
    /// When a recording starts, and its files' path (without their extensions), given the base directory.
    fn base(&self, base_directory: Option<&crate::core::base_directory::Place>) -> (DateTime<Local>, PathBuf) {
        let start = Local::now();
        (start, self.location.directory(base_directory).join(recording_name(base_directory, &self.prefix, &start)))
    }
}

impl Streams {
    fn of(inputs: &[Option<DataStream>]) -> Self {
        let events = |index: usize| match inputs.get(index) {
            Some(Some(DataStream::Events(events))) => Some(*events),
            _ => None,
        };
        let mut streams = Streams { events: events(3), position_events: events(4), ..Default::default() };
        for input in inputs.iter().flatten() {
            match *input {
                DataStream::Spikes(s) => streams.spikes = Some(s),
                DataStream::Continuous(c) => streams.continuous = Some(c),
                DataStream::Positions(p) => streams.positions = Some(p),
                DataStream::LogLines(l) => streams.log = Some(l),
                _ => {}
            }
        }
        streams
    }
}

/// The settings recorded with the data, from the controls.
#[derive(Clone, Default)]
struct Recorded {
    detection: Option<Detection>,
    gains: Gains,
    /// Which channels the EEG's are, and their gains.
    eeg_selection: Option<ChannelSelection>,
    eeg_gains: Gains,
}

impl Recorded {
    fn of(controls: &[Option<Setting>]) -> Self {
        let mut recorded = Recorded::default();
        if let Some(Some(Setting::Detection(detection))) = controls.first() {
            recorded.detection = Some(detection.clone());
        }
        if let Some(Some(Setting::Gains(gains))) = controls.get(1) {
            recorded.gains = gains.clone();
        }
        if let Some(Some(Setting::ChannelSelection(selection))) = controls.get(2) {
            recorded.eeg_selection = Some(selection.clone());
        }
        if let Some(Some(Setting::Gains(gains))) = controls.get(3) {
            recorded.eeg_gains = gains.clone();
        }
        recorded
    }

    /// Which channel (upstream, counting from 1) each of `count` EEG channels is: as the selection passes them on,
    /// if it's applied, otherwise as they come.
    fn eeg_channels(&self, count: u32) -> Vec<u32> {
        let selected = self
            .eeg_selection
            .as_ref()
            .filter(|s| s.apply)
            .map(|s| s.channels.trim())
            .filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("all"));
        let channels =
            selected.and_then(|text| parse_channels(text, u32::MAX / 2).ok()).filter(|c| c.len() == count as usize);
        channels.unwrap_or_else(|| (0..count).collect()).into_iter().map(|c| c + 1).collect()
    }
}

/// A gain in the Axona format's terms: what gives the range a byte's samples (±128) cover.
fn axona_gain(gain: f32) -> f32 {
    (FULL_SCALE_UV * gain / 128.0 * 10.0).round() / 10.0
}

/// Copies a pass's inputs into `chunk`: of the continuous data, only what the EEG keeps, every `step`th sample
/// (counting from the `continuous_samples` so far).
fn copy(pass: &Pass<Edges>, chunk: &mut Chunk, step: Option<usize>, continuous_samples: &mut u64) {
    chunk.spikes.clear();
    chunk.waveforms.clear();
    chunk.spike_pass_samples = 0;
    if let Some(Some(Buffer::Spikes(spikes))) = pass.inputs.first() {
        chunk.spikes.extend(spikes.spikes.iter().map(|s| (s.group, s.sample)));
        chunk.waveforms.extend_from_slice(&spikes.waveforms);
        chunk.spike_pass_samples = spikes.pass_samples;
    }
    chunk.continuous.clear();
    chunk.continuous_len = 0;
    if let (Some(Some(Buffer::Continuous(continuous))), Some(step)) = (pass.inputs.get(1), step) {
        let first = keeping_from(*continuous_samples, step);
        for channel in 0..continuous.channels {
            chunk.continuous.extend(continuous.channel(channel).iter().skip(first).step_by(step));
        }
        chunk.continuous_len = continuous.len.saturating_sub(first).div_ceil(step);
        *continuous_samples += continuous.len as u64;
    }
    chunk.positions.clear();
    if let Some(Some(Buffer::Positions(positions))) = pass.inputs.get(2) {
        chunk.positions.extend_from_slice(&positions.samples);
    }
    chunk.events.clear();
    chunk.event_pass_samples = 0;
    if let Some(Some(Buffer::Events(events))) = pass.inputs.get(3) {
        chunk.events.extend_from_slice(&events.events);
        chunk.event_pass_samples = events.pass_samples;
    }
    chunk.position_events.clear();
    chunk.position_event_pass_samples = 0;
    if let Some(Some(Buffer::Events(events))) = pass.inputs.get(4) {
        chunk.position_events.extend_from_slice(&events.events);
        chunk.position_event_pass_samples = events.pass_samples;
    }
    chunk.log.clear();
    chunk.log_lines.clear();
    if let Some(Some(Buffer::LogLines(log))) = pass.inputs.get(5) {
        for (time, line) in log.lines() {
            chunk.log.extend_from_slice(line);
            chunk.log_lines.push((time, line.len() as u32));
        }
    }
}

/// The Axona format's full scale for a gain of 1, in µV: its `.set` files give each channel's gain `g` for spikes
/// recorded over ±`FULL_SCALE_UV / g` (as 8-bit samples), and the full scale as `ADC_fullscale_mv`.
pub const FULL_SCALE_UV: f32 = 1_500_000.0;

/// How many samples to step for each 5 kHz sample, if the rate's a multiple of 5 kHz.
/// The first of the next samples to keep, keeping every `step`th, with `so_far` before them.
fn keeping_from(so_far: u64, step: usize) -> usize {
    (step - (so_far % step as u64) as usize) % step
}

fn egf_step(rate: f32) -> Option<usize> {
    let rate = rate.round() as u32;
    (rate >= EGF_RATE && rate.is_multiple_of(EGF_RATE)).then_some((rate / EGF_RATE) as usize)
}

/// A value as a signed byte, saturating.
fn to_i8(value: f32) -> u8 {
    (value.round() as i32).clamp(-128, 127) as i8 as u8
}

/// Seconds as ticks of the timebase.
fn ticks(seconds: f64) -> [u8; 4] {
    ((seconds * TIMEBASE).round().clamp(0.0, i32::MAX as f64) as i32).to_be_bytes()
}

/// An LTX file: header lines, then the data, with the duration (and one count) filled in at the end.
struct LtxFile {
    path: PathBuf,
    file: BufWriter<File>,
    /// Where the duration's value goes, and the count's.
    duration_at: u64,
    count_at: Option<u64>,
    in_data: bool,
}

/// Room for a value filled in at the end.
const PLACEHOLDER: &str = "              ";

impl LtxFile {
    fn create(path: &Path, start: &DateTime<Local>) -> io::Result<Self> {
        let mut file = BufWriter::new(File::create(path)?);
        write!(file, "trial_date {}", start.format("%A, %d %b %Y"))?;
        write!(file, "\r\ntrial_time {}", start.format("%H:%M"))?;
        write!(file, "\r\ncreated_by {}", crate::core::full_version())?;
        write!(file, "\r\nduration ")?;
        let duration_at = file.stream_position()?;
        write!(file, "{PLACEHOLDER}")?;
        Ok(LtxFile { path: path.to_path_buf(), file, duration_at, count_at: None, in_data: false })
    }

    fn header(&mut self, key: &str, value: impl std::fmt::Display) -> io::Result<()> {
        write!(self.file, "\r\n{key} {value}")
    }

    /// A header whose value (a count) is filled in at the end.
    fn count_header(&mut self, key: &str) -> io::Result<()> {
        write!(self.file, "\r\n{key} ")?;
        self.count_at = Some(self.file.stream_position()?);
        write!(self.file, "{PLACEHOLDER}")
    }

    fn data(&mut self, bytes: &[u8]) -> io::Result<()> {
        if !self.in_data {
            write!(self.file, "\r\ndata_start")?;
            self.in_data = true;
        }
        self.file.write_all(bytes)
    }

    /// Fills in what's only known at the end, and closes it, returning its path.
    fn finish(mut self, seconds: u64, count: u64) -> io::Result<PathBuf> {
        if self.in_data {
            write!(self.file, "\r\ndata_end")?;
        }
        self.file.seek(SeekFrom::Start(self.duration_at))?;
        write!(self.file, "{seconds}")?;
        if let Some(at) = self.count_at {
            self.file.seek(SeekFrom::Start(at))?;
            write!(self.file, "{count}")?;
        }
        self.file.flush()?;
        Ok(self.path)
    }
}

/// How positions are stored: relative to the crop's bounding box (the frame, without a crop), scaled down if need
/// be to fit 0–1022.
#[derive(Clone, Copy)]
struct Encoding {
    origin: [f32; 2],
    scale: f32,
}

impl Encoding {
    fn coordinate(&self, value: f32, axis: usize) -> u16 {
        if !value.is_finite() {
            return MISSING;
        }
        ((value - self.origin[axis]) * self.scale).round().clamp(0.0, (MISSING - 1) as f32) as u16
    }
}

/// The crop's bounding box (min x, min y, max x, max y), or the frame's, in pixels.
fn window(crop: Option<&Crop>, frame: [u32; 2]) -> [f32; 4] {
    crop.and_then(Crop::bounds).unwrap_or([0.0, 0.0, frame[0] as f32, frame[1] as f32])
}

struct Tetrode {
    file: LtxFile,
    spikes: u64,
}

struct Egf {
    file: LtxFile,
    samples: u64,
}

struct Pos {
    file: LtxFile,
    encoding: Encoding,
    samples: u64,
}

/// A recording's LTX files, written on the writer's thread.
struct LtxFiles {
    start: DateTime<Local>,
    streams: Streams,
    set: LtxFile,
    tetrodes: Vec<Tetrode>,
    egf: Vec<Egf>,
    /// Samples of continuous data so far (for which to keep at 5 kHz), and of the spikes' and events' passes.
    spike_samples: i64,
    event_samples: u64,
    position_event_samples: u64,
    /// The source's time of the first position, which positions' times, the timing LED's and log lines' count from.
    first_position: Option<f64>,
    pos: Option<Pos>,
    ttl: Option<LtxFile>,
    pos_ttl: Option<LtxFile>,
    log: Option<BufWriter<File>>,
    log_path: PathBuf,
    /// For each record of `bytes`.
    record: Vec<u8>,
}

impl LtxFiles {
    fn open(base: &Path, start: DateTime<Local>, streams: Streams, recorded: Recorded) -> io::Result<Self> {
        if let Some(parent) = base.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = |extension: &str| -> io::Result<LtxFile> {
            let mut path: PathBuf = base.to_path_buf();
            path.as_mut_os_string().push(extension);
            LtxFile::create(&path, &start)
        };
        let mut set = file(".set")?;
        set.header("lasttrialdatetime", start.timestamp())?;
        for (i, bearing) in [0, 180, 0, 0].into_iter().enumerate() {
            set.header(&format!("lightBearing_{}", i + 1), bearing)?;
        }
        for (i, active) in [1, 1, 0, 0].into_iter().enumerate() {
            set.header(&format!("colactive_{}", i + 1), active)?;
        }

        let mut tetrodes = Vec::new();
        if let Some(spikes) = streams.spikes {
            set.header("ADC_fullscale_mv", FULL_SCALE_UV / 1000.0)?;
            for channel in 0..spikes.channels as usize {
                set.header(&format!("gain_ch_{channel}"), axona_gain(recorded.gains.get(channel)))?;
            }
            for group in 0..spikes.groups() {
                let mut f = file(&format!(".{}", group + 1))?;
                f.header("num_chans", SPIKE_CHANNELS)?;
                f.header("bytes_per_timestamp", 4)?;
                f.header("samples_per_spike", SPIKE_SAMPLES)?;
                f.header("bytes_per_sample", 1)?;
                f.header("spike_format", "t,ch1,t,ch2,t,ch3,t,ch4")?;
                f.header("sample_rate", format!("{:.6} hz", spikes.sample_rate))?;
                f.header("timebase", format!("{TIMEBASE} hz"))?;
                f.count_header("num_spikes")?;
                tetrodes.push(Tetrode { file: f, spikes: 0 });
            }
        }

        let mut egf = Vec::new();
        if let Some(continuous) = streams.continuous.filter(|c| egf_step(c.sample_rate).is_some()) {
            if streams.spikes.is_none() {
                set.header("ADC_fullscale_mv", FULL_SCALE_UV / 1000.0)?;
            }
            let numbers = recorded.eeg_channels(continuous.channels);
            for channel in 0..continuous.channels {
                let (number, gain) = (numbers[channel as usize], recorded.eeg_gains.get(channel as usize));
                set.header(&format!("EEG_ch_{}", channel + 1), number)?;
                set.header(&format!("EEG_gain_{}", channel + 1), axona_gain(gain))?;
                let mut f = file(&if channel == 0 { ".egf".into() } else { format!(".egf{}", channel + 1) })?;
                f.header("num_chans", 1)?;
                f.header("sample_rate", format!("{EGF_RATE} hz"))?;
                f.header("stream_name", "eeg")?;
                f.header("chan_name", format!("CH{number}"))?;
                f.header("gain", axona_gain(gain))?;
                f.header("range_uv", format!("{:.1}", recorded.eeg_gains.range_uv(channel as usize)))?;
                f.count_header("num_EEG_samples")?;
                egf.push(Egf { file: f, samples: 0 });
            }
        }

        let pos = match streams.positions {
            Some(positions) => Some(open_pos(file(".pos")?, positions, recorded.detection.as_ref())?),
            None => None,
        };
        let ttl = match streams.events {
            Some(_) => Some(open_ttl(file(".ttl")?, "digital_inputs", "recording_start")?),
            None => None,
        };
        // Connected, it only has anything to say if there's a timing LED, as far as we can tell.
        let timing_led = recorded.detection.as_ref().map(|d| d.timing_led);
        let pos_ttl = match streams.position_events {
            Some(_) if timing_led.is_none_or(|led| led.enabled) => {
                let mut f = open_ttl(file(".pos.ttl")?, "timing_led", "first_position")?;
                if let Some(detection) = recorded.detection.as_ref().filter(|_| timing_led.is_some()) {
                    let led = detection.timing_led;
                    f.header("timing_led_x", led.center[0])?;
                    f.header("timing_led_y", led.center[1])?;
                    f.header("timing_led_radius", led.radius)?;
                    f.header("timing_led_threshold", detection.timing_threshold())?;
                    f.header("timing_led_min_area", detection.timing_min_area())?;
                }
                Some(f)
            }
            _ => None,
        };
        let mut log_path = base.to_path_buf();
        log_path.as_mut_os_string().push(".txt");
        let log = match streams.log {
            Some(_) => Some(BufWriter::new(File::create(&log_path)?)),
            None => None,
        };
        log::info!("Recording LTX to {}", base.display());
        Ok(LtxFiles {
            start,
            streams,
            set,
            tetrodes,
            egf,
            spike_samples: 0,
            event_samples: 0,
            position_event_samples: 0,
            first_position: None,
            pos,
            ttl,
            pos_ttl,
            log,
            log_path,
            record: Vec::new(),
        })
    }
}

/// A TTL file's headers, saying how to read its lines: what its changes are of, and what their seconds count from.
fn open_ttl(mut f: LtxFile, source: &str, seconds_from: &str) -> io::Result<LtxFile> {
    f.header("ttl_format", "ttl_<line> <seconds> <state>")?;
    f.header("ttl_state_1", "up")?;
    f.header("ttl_state_0", "down")?;
    f.header("ttl_source", source)?;
    f.header("ttl_seconds_from", seconds_from)?;
    Ok(f)
}

/// `text`, with any line breaks as spaces.
fn on_one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

/// The position file's headers: what's needed to read the positions, and the detector's settings.
fn open_pos(mut f: LtxFile, positions: Positions, detection: Option<&Detection>) -> io::Result<Pos> {
    let crop = detection.map(|d| &d.crop).filter(|c| **c != Crop::None);
    let [min_x, min_y, max_x, max_y] = window(crop, [positions.width, positions.height]);
    let (width, height) = (max_x - min_x, max_y - min_y);
    let scale = (((MISSING - 1) as f32) / width.max(height).max(1.0)).min(1.0);
    let ppm = detection.and_then(Detection::pixels_per_metre);
    let fps = format!("{:.6} hz", positions.fps);
    f.header("timestamp_timebase", format!("{TIMEBASE} hz"))?;
    f.header("timebase", &fps)?;
    f.header("sample_rate", &fps)?;
    f.header("bytes_per_timestamp", 4)?;
    f.header("bytes_per_coord", 2)?;
    f.header("pos_format", "t,x1,y1,x2,y2,numpix1,numpix2")?;
    f.header("num_colours", 4)?;
    for (i, bearing) in [330, 150, 0, 0].into_iter().enumerate() {
        f.header(&format!("bearing_colour_{}", i + 1), bearing)?;
    }
    let source = positions.source;
    f.header("position_metadata_source", source)?;
    f.header("data_source", crate::core::full_version())?;
    f.header("pixels_per_metre", ppm.map_or(f32::NAN, |p| p * scale))?;
    let (right, bottom) = ((width * scale).round(), (height * scale).round());
    for (key, value) in [("min_x", 0.0), ("max_x", right), ("min_y", 0.0), ("max_y", bottom)] {
        f.header(&format!("window_{key}"), value)?;
    }
    for (key, value) in [("min_x", 0.0), ("max_x", right), ("min_y", 0.0), ("max_y", bottom)] {
        f.header(key, value)?;
    }
    if let Some(ppm) = ppm {
        f.header(&format!("{source}.pixels_per_metre"), ppm)?;
    }
    f.header(&format!("{source}.window"), format!("{min_x} {min_y} {max_x} {max_y}"))?;
    if let Some(crop) = crop {
        f.header(&format!("{source}.crop"), format!("{crop:?}"))?;
    }
    f.header(&format!("{source}.frame_size"), format!("{} {}", positions.width, positions.height))?;
    let transformed = scale < 1.0 || min_x != 0.0 || min_y != 0.0;
    f.header("position_coordinate_transform", if transformed { "crop_relative_uniform_scale" } else { "identity" })?;
    f.header("position_coordinate_origin_x", min_x)?;
    f.header("position_coordinate_origin_y", min_y)?;
    f.header("position_coordinate_scale", scale)?;
    f.header("position_missing_coordinate", MISSING)?;
    f.header("position_area_units", "camera_pixels_saturated_uint16")?;
    for (key, value) in detection.iter().flat_map(|d| &d.custom) {
        f.header(&on_one_line(key), on_one_line(value))?;
    }
    f.count_header("num_pos_samples")?;
    Ok(Pos { file: f, encoding: Encoding { origin: [min_x, min_y], scale }, samples: 0 })
}

impl Files<Chunk> for LtxFiles {
    fn write(&mut self, chunk: &Chunk) -> io::Result<u64> {
        let mut bytes = 0;
        if let Some(spikes) = self.streams.spikes {
            let (k, samples) = (spikes.group_size as usize, spikes.waveform_samples() as usize);
            for (i, &(group, sample)) in chunk.spikes.iter().enumerate() {
                let Some(tetrode) = self.tetrodes.get_mut(group as usize) else { continue };
                let seconds = (self.spike_samples + sample as i64) as f64 / spikes.sample_rate as f64;
                let waveform = &chunk.waveforms[i * k * samples..(i + 1) * k * samples];
                self.record.clear();
                for channel in 0..SPIKE_CHANNELS {
                    self.record.extend_from_slice(&ticks(seconds));
                    let values = waveform.get(channel * samples..(channel + 1) * samples).unwrap_or(&[]);
                    // Already int8 values (from a Spike Gain), as floats: just made integers.
                    let start = self.record.len();
                    self.record.extend(values.iter().take(SPIKE_SAMPLES).map(|&v| to_i8(v)));
                    self.record.resize(start + SPIKE_SAMPLES, 0);
                }
                tetrode.file.data(&self.record)?;
                tetrode.spikes += 1;
                bytes += self.record.len() as u64;
            }
            self.spike_samples += chunk.spike_pass_samples as i64;
        }

        if chunk.continuous_len > 0 {
            let len = chunk.continuous_len;
            for (channel, egf) in self.egf.iter_mut().enumerate() {
                let values = &chunk.continuous[channel * len..(channel + 1) * len];
                self.record.clear();
                // Already int8 values (from an Continuous Gain), as floats: just made integers.
                self.record.extend(values.iter().map(|&v| to_i8(v)));
                egf.file.data(&self.record)?;
                egf.samples += self.record.len() as u64;
                bytes += self.record.len() as u64;
            }
        }

        if let Some(pos) = &mut self.pos {
            for sample in &chunk.positions {
                let first = *self.first_position.get_or_insert(sample.time);
                self.record.clear();
                self.record.extend_from_slice(&ticks(sample.time - first));
                let led = |i: usize| sample.leds[i].map_or([f32::NAN, f32::NAN, 0.0], |b| [b.x, b.y, b.area as f32]);
                let (a, b) = (led(0), led(1));
                let values = [
                    pos.encoding.coordinate(a[0], 0),
                    pos.encoding.coordinate(a[1], 1),
                    pos.encoding.coordinate(b[0], 0),
                    pos.encoding.coordinate(b[1], 1),
                    a[2].min(u16::MAX as f32) as u16,
                    b[2].min(u16::MAX as f32) as u16,
                    0,
                    0,
                ];
                for value in values {
                    self.record.extend_from_slice(&value.to_be_bytes());
                }
                pos.file.data(&self.record)?;
                pos.samples += 1;
                bytes += self.record.len() as u64;
            }
        }

        if let (Some(ttl), Some(events)) = (&mut self.ttl, self.streams.events) {
            for event in &chunk.events {
                let seconds = (self.event_samples + event.sample as u64) as f64 / events.sample_rate as f64;
                let line = format!("ttl_{} {seconds} {}\r\n", event.line + 1, event.high as u8);
                ttl.data(line.as_bytes())?;
                bytes += line.len() as u64;
            }
            self.event_samples += chunk.event_pass_samples as u64;
        }

        if let (Some(ttl), Some(events)) = (&mut self.pos_ttl, self.streams.position_events) {
            for event in &chunk.position_events {
                let seconds = match event.time {
                    Some(time) => time - *self.first_position.get_or_insert(time),
                    None => (self.position_event_samples + event.sample as u64) as f64 / events.sample_rate as f64,
                };
                let line = format!("ttl_{} {seconds:.6} {}\r\n", event.line + 1, event.high as u8);
                ttl.data(line.as_bytes())?;
                bytes += line.len() as u64;
            }
            self.position_event_samples += chunk.position_event_pass_samples as u64;
        }

        if let Some(log) = &mut self.log {
            let mut text = chunk.log.as_slice();
            for &(time, length) in &chunk.log_lines {
                let (line, rest) = text.split_at(length as usize);
                text = rest;
                let seconds = time - *self.first_position.get_or_insert(time);
                self.record.clear();
                write!(self.record, "{seconds:.6} ")?;
                self.record.extend_from_slice(line);
                self.record.extend_from_slice(b"\r\n");
                log.write_all(&self.record)?;
                bytes += self.record.len() as u64;
            }
        }
        Ok(bytes)
    }

    fn finish(self: Box<Self>) -> io::Result<Vec<PathBuf>> {
        let seconds = (Local::now() - self.start).num_seconds().max(0) as u64;
        let mut paths = vec![self.set.finish(seconds, 0)?];
        for tetrode in self.tetrodes {
            paths.push(tetrode.file.finish(seconds, tetrode.spikes)?);
        }
        for egf in self.egf {
            paths.push(egf.file.finish(seconds, egf.samples)?);
        }
        if let Some(pos) = self.pos {
            paths.push(pos.file.finish(seconds, pos.samples)?);
        }
        for ttl in [self.ttl, self.pos_ttl].into_iter().flatten() {
            paths.push(ttl.finish(seconds, 0)?);
        }
        if let Some(mut log) = self.log {
            log.flush()?;
            paths.push(self.log_path);
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard_components::spikes::Spike;
    use crate::standard_components::tracking::pipeline::Blob;
    use crate::standard_components::tracking::{CAMERA_TRACKER, TimingLed};

    fn read(path: &Path, extension: &str) -> Vec<u8> {
        let mut path = path.to_path_buf();
        path.as_mut_os_string().push(extension);
        std::fs::read(path).unwrap()
    }

    /// The header's value for `key`.
    fn header(bytes: &[u8], key: &str) -> String {
        let text = String::from_utf8_lossy(bytes);
        let line = text.split("\r\n").find(|l| l.starts_with(&format!("{key} "))).expect(key);
        line[key.len() + 1..].trim().to_string()
    }

    /// The bytes between `data_start` and `data_end`.
    fn data(bytes: &[u8]) -> &[u8] {
        let find = |token: &[u8]| bytes.windows(token.len()).position(|w| w == token).unwrap();
        &bytes[find(b"data_start") + 10..find(b"\r\ndata_end")]
    }

    #[test]
    fn the_eeg_keeps_every_step_th_sample_across_passes() {
        // Keeping every 6th of passes 4, 9 and 5 samples long: samples 0, 6 and 12 (of 18).
        let (mut so_far, mut kept) = (0u64, Vec::new());
        for len in [4usize, 9, 5] {
            let first = keeping_from(so_far, 6);
            kept.extend((first..len).step_by(6).map(|t| so_far + t as u64));
            assert_eq!(len.saturating_sub(first).div_ceil(6), (first..len).step_by(6).count());
            so_far += len as u64;
        }
        assert_eq!(kept, vec![0, 6, 12]);
    }

    #[test]
    fn writes_ups_and_downs_saying_which_is_which_and_the_timing_led_by_the_positions_clock() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-ltx-ttl-{}", std::process::id()));
        let base = dir.join("test");
        let events = Events { lines: 8, sample_rate: 1000.0 };
        let streams = Streams {
            positions: Some(Positions { fps: 50.0, width: 640, height: 480, source: CAMERA_TRACKER }),
            events: Some(events),
            position_events: Some(Events { lines: 1, sample_rate: 50.0 }),
            ..Default::default()
        };
        let timing_led = TimingLed { enabled: true, center: [600.0, 30.0], radius: 8.0, ..Default::default() };
        let detection = Some(Detection { timing_led, ..Default::default() });
        let recorded = Recorded { detection, ..Default::default() };
        let mut files = Box::new(LtxFiles::open(&base, Local::now(), streams, recorded).unwrap());

        let sample = |time| PositionSample {
            frame: 0,
            time,
            leds: [None, None],
            timing_led: Some(true),
            pixels_per_metre: f32::NAN,
        };
        let event = |sample, line, high, time| Event { sample, line, high, time };
        // Positions start at 100 s by the camera's clock; the LED comes on 0.25 s later, and goes off 0.3 s after that.
        let mut chunk = Chunk { event_pass_samples: 20, ..Default::default() };
        chunk.positions.push(sample(100.0));
        chunk.events.push(event(10, 1, true, None));
        chunk.events.push(event(15, 1, false, None));
        files.write(&chunk).unwrap();
        chunk.events.clear();
        chunk.positions[0] = sample(100.25);
        chunk.position_events.push(event(0, 0, true, Some(100.25)));
        chunk.position_events.push(event(0, 0, false, Some(100.55)));
        files.write(&chunk).unwrap();
        files.finish().unwrap();

        let ttl = read(&base, ".ttl");
        assert_eq!(data(&ttl), b"ttl_2 0.01 1\r\nttl_2 0.015 0\r\n", "both ups and downs");
        assert_eq!(header(&ttl, "ttl_format"), "ttl_<line> <seconds> <state>");
        assert_eq!((header(&ttl, "ttl_state_1"), header(&ttl, "ttl_state_0")), ("up".into(), "down".into()));
        assert_eq!(header(&ttl, "ttl_source"), "digital_inputs");
        let pos_ttl = read(&base, ".pos.ttl");
        assert_eq!(data(&pos_ttl), b"ttl_1 0.250000 1\r\nttl_1 0.550000 0\r\n", "from the first position");
        assert_eq!(header(&pos_ttl, "ttl_source"), "timing_led");
        assert_eq!(header(&pos_ttl, "ttl_seconds_from"), "first_position");
        assert_eq!(header(&pos_ttl, "timing_led_x"), "600");
        assert_eq!(header(&pos_ttl, "timing_led_threshold"), "200");

        // Without a timing LED, connected or not, there's no file.
        let base = dir.join("no led");
        let recorded = Recorded { detection: Some(Detection::default()), ..Default::default() };
        Box::new(LtxFiles::open(&base, Local::now(), streams, recorded).unwrap()).finish().unwrap();
        let mut path = base.clone();
        path.as_mut_os_string().push(".pos.ttl");
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn positions_from_elsewhere_say_so_with_their_custom_settings_ttls_and_log() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-ltx-elsewhere-{}", std::process::id()));
        let base = dir.join("test");
        let streams = Streams {
            positions: Some(Positions { fps: 50.0, width: 1000, height: 1000, source: "vr_computer" }),
            position_events: Some(Events { lines: 1, sample_rate: 1000.0 }),
            log: Some(LogLines { width: 32 }),
            ..Default::default()
        };
        let custom = [("maze.arm length", "3"), ("note", "two\r\nlines")].map(|(k, v)| (k.to_string(), v.to_string()));
        let crop = Crop::Rectangle { center: [100.0, 50.0], width: 200.0, height: 100.0, angle: 0.0 };
        let timing_led = TimingLed { enabled: true, radius: 0.0, ..Default::default() };
        let detection = Detection { crop, timing_led, custom: custom.into(), ..Default::default() };
        let recorded = Recorded { detection: Some(detection.clone()), ..Default::default() };
        let mut files = Box::new(LtxFiles::open(&base, Local::now(), streams, recorded).unwrap());
        let mut chunk = Chunk::default();
        chunk.positions.push(PositionSample {
            frame: 0,
            time: 1.5,
            leds: [None, None],
            timing_led: None,
            pixels_per_metre: f32::NAN,
        });
        chunk.position_events.push(Event { sample: 0, line: 0, high: true, time: Some(2.0) });
        chunk.log.extend_from_slice(b"first linesecond");
        chunk.log_lines.extend([(1.75, 10), (4.0, 6)]);
        files.write(&chunk).unwrap();
        files.finish().unwrap();

        let pos = read(&base, ".pos");
        assert_eq!(header(&pos, "maze.arm length"), "3", "a key as it came");
        assert_eq!(header(&pos, "note"), "two  lines", "a value on one line");
        assert_eq!(header(&pos, "position_metadata_source"), "vr_computer");
        assert_eq!(header(&pos, "vr_computer.window"), "0 0 200 100");
        assert_eq!(header(&pos, "position_coordinate_scale"), "1", "pixels, as a camera's are");
        assert_eq!((header(&pos, "window_max_x"), header(&pos, "window_max_y")), ("200".into(), "100".into()));
        let pos_ttl = read(&base, ".pos.ttl");
        assert_eq!(header(&pos_ttl, "timing_led_radius"), "0", "as the detection says");
        assert_eq!(data(&pos_ttl), b"ttl_1 0.500000 1\r\n", "from the first position");
        assert_eq!(read(&base, ".txt"), b"0.250000 first line\r\n2.500000 second\r\n", "no headers, and from it too");

        // Saying there's no TTL coming, there's no file, as with a camera's timing LED.
        let base = dir.join("no ttl");
        let detection = Detection { timing_led: TimingLed { enabled: false, ..timing_led }, ..detection };
        let recorded = Recorded { detection: Some(detection), ..Default::default() };
        Box::new(LtxFiles::open(&base, Local::now(), streams, recorded).unwrap()).finish().unwrap();
        let mut path = base.clone();
        path.as_mut_os_string().push(".pos.ttl");
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writes_spikes_eeg_and_positions() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-ltx-{}", std::process::id()));
        let base = dir.join("test");
        let spikes =
            Spikes { channels: 4, sample_rate: 30000.0, group_size: 4, pre: 8, post: 32, polarity: Default::default() };
        let streams = Streams {
            spikes: Some(spikes),
            continuous: Some(Continuous { channels: 2, sample_rate: 30000.0 }),
            positions: Some(Positions { fps: 50.0, width: 640, height: 480, source: CAMERA_TRACKER }),
            events: None,
            position_events: None,
            log: None,
        };
        // Gains recorded: a µV a step on the first channel, half a µV on the second.
        let mut gains = Gains::default();
        let channels = crate::standard_components::spikes::Channels { count: 4, group_size: 4 };
        gains.set_range(0, 128.0, channels);
        gains.set_range(1, 64.0, channels);
        // The EEG: channels 5 and 7 of the board's, at ±200 µV.
        let mut eeg_gains = Gains { scope: crate::standard_components::spikes::GainScope::All, ..Default::default() };
        eeg_gains.set_range(0, 200.0, crate::standard_components::spikes::Channels { count: 2, group_size: 1 });
        let eeg_selection = Some(ChannelSelection { apply: true, channels: "5, 7".into() });
        let recorded = Recorded { detection: None, gains, eeg_selection, eeg_gains };
        let mut files = Box::new(LtxFiles::open(&base, Local::now(), streams, recorded).unwrap());

        // A spike 300 samples into the second pass of 600: at 900 samples, 30 ms, 2880 ticks.
        let mut chunk = Chunk { spike_pass_samples: 600, ..Default::default() };
        files.write(&chunk).unwrap();
        let spike = Spike { sample: 300, group: 0, channel: 0 };
        chunk.spikes.push((spike.group, spike.sample));
        // As int8 values (from a Spike Gain), from -80.
        chunk.waveforms = (0..160).map(|i| i as f32 - 80.0).collect();
        // Two samples kept on each channel (at 5 kHz), as int8 values.
        chunk.continuous = vec![0.0, 30.0, 60.0, 90.0];
        chunk.continuous_len = 2;
        let blob = Blob { x: 100.0, y: 50.0, area: 30 };
        chunk.positions.push(PositionSample {
            frame: 1,
            time: 5.0,
            leds: [Some(blob), None],
            timing_led: None,
            pixels_per_metre: f32::NAN,
        });
        files.write(&chunk).unwrap();
        files.finish().unwrap();

        let tetrode = read(&base, ".1");
        assert_eq!(header(&tetrode, "num_spikes"), "1");
        assert_eq!(header(&tetrode, "created_by"), crate::core::full_version(), "which build wrote it");
        let record = data(&tetrode);
        assert_eq!(record.len(), 4 * 54);
        assert_eq!(i32::from_be_bytes(record[..4].try_into().unwrap()), 2880);
        // Each channel's first sample, as it came.
        assert_eq!(record[4] as i8, -80);
        assert_eq!(record[54 + 4] as i8, -40);
        assert_eq!(record[2 * 54 + 4] as i8, 0);
        assert_eq!(record[3 * 54 + 4] as i8, 40);
        let egf = read(&base, ".egf2");
        assert_eq!(header(&egf, "num_EEG_samples"), "2");
        assert_eq!(data(&egf).iter().map(|b| *b as i8).collect::<Vec<_>>(), vec![60, 90]);
        assert_eq!((header(&egf, "chan_name"), header(&egf, "range_uv")), ("CH7".into(), "200.0".into()));
        let pos = read(&base, ".pos");
        let sample = data(&pos);
        assert_eq!(u16::from_be_bytes([sample[4], sample[5]]), 100);
        assert_eq!(u16::from_be_bytes([sample[8], sample[9]]), MISSING, "no second LED");
        assert_eq!(u16::from_be_bytes([sample[12], sample[13]]), 30);
        let set = read(&base, ".set");
        assert!(set.windows(10).all(|w| w != b"data_start"), "the set file is headers only");
        assert_eq!(header(&set, "ADC_fullscale_mv"), "1500");
        assert_eq!(header(&set, "gain_ch_1"), "23437.5");
        assert_eq!(header(&set, "gain_ch_3"), "6000");
        assert_eq!((header(&set, "EEG_ch_1"), header(&set, "EEG_ch_2")), ("5".into(), "7".into()));
        assert_eq!(header(&set, "EEG_gain_1"), "7500");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
