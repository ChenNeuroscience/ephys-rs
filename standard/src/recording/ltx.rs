//! Write LTX: recording in the LTX format, as the Open Ephys LTX plugin writes it, for legacy tetrode analysis code
//! (and Waveform GUI). For a recording called `name` (the prefix and when it started):
//!
//! - `name.set`: headers only.
//! - `name.1`, `name.2`...: each tetrode's spikes, each as 4 × (a 4-byte timestamp, then 50 one-byte samples).
//! - `name.egf`, `name.egf2`...: each continuous channel at 5 kHz, a byte a sample.
//! - `name.pos`: LED positions, a 4-byte timestamp then 8 two-byte values (x1, y1, x2, y2, and the LEDs' sizes).
//! - `name.ttl`: digital input changes, as text.
//! - `name.pos.ttl`: the timing LED's changes, likewise, from its `pos ttl` input, if there's a timing LED.
//! - `name.txt`: log lines, with no headers: each as `<seconds> <line>`.
//!
//! Each file but `name.txt` is `key value` header lines, then the data between `data_start` and `data_end`; counts and
//! the duration (from the first pass recorded) are filled in at the end, and the `.set` file's `recording_requested`
//! (when Record was pressed, which names the files) and `recording_effective` (its first pass), to the millisecond.
//! Timestamps count 96 kHz ticks, and multi-byte values are big-endian.
//!
//! Times count from when recording started by the clock the data is timed by (see `crate::standard::clock`). Spikes,
//! the EEG and digital inputs should all be timed by the board's clock, so that they line up exactly. For the board's
//! clock, recording started at the start of the board's `time` in the first pass recorded, if the `time` input is
//! connected, or else at the earliest of the spikes, EEG and digital inputs in that pass. The `.egf` files hold samples
//! without times, so the writer places the EEG's samples by their times: samples from before the start are skipped,
//! and a gap is filled with zeros. Positions, the timing LED's changes and log lines are usually timed by another
//! clock (the camera's or the VR computer's), so count from the first of them by that clock, usually the first
//! position. Spikes' samples are
//! -128 to 127, covering the full scale: their waveforms should come amplified by a Spike Gain (see `Gains`), whose
//! gains (connected, read-only, to `gains`) are recorded in the `.set` file, in Axona's terms: its full scale, and the
//! gains that fit the same over it (`ADC_fullscale_mv`, `gain_ch_0`...). The EEG (its `eeg` input) should come at 5 kHz
//! (from a Downsample), amplified too, by a Continuous Gain, whose gains (connected, read-only, to `eeg gains`) are
//! recorded likewise, as `EEG_gain_1`... in the `.set` file, and `gain` in each `.egf` file. Which channel each EEG
//! channel is (from the Channel Selector choosing them, connected to `eeg selection`) is recorded as `EEG_ch_1`... in
//! the `.set` file, and `chan_name` in each `.egf` file.
//!
//! The positions' header records the position detector's settings (connected, read-only, to `detection`): the crop,
//! whose bounding box positions are measured from (scaled down to fit 0–1022 if need be; 1023 means no LED), and the
//! calibration. And it records anything in its `custom` settings as headers of their own, as they are (whatever a
//! reader makes of a key with a space in it, or one that's the same as another header), except that line breaks are
//! written as spaces, as a header's a line.

use ephys_rs::engine::{
    Buffers, DataStreamInPorts, NoPorts, PortForm, Prepare, PrepareContext, Prepared, TreeShake, UserDecision,
};
use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use eframe::egui;
use serde::{Deserialize, Serialize};

use super::{
    Files, OnFinished, Origins, Status, Writing, after_recording, location_summary, recording_path, show_status,
    status_ui,
};
use crate::standard::clock::{ClockId, Time};
use crate::standard::ephys::reference::parse_channels;
use crate::standard::ephys::{ChannelSelection, Continuous, Events};
use crate::standard::logs::LogLines;
use crate::standard::other::run_command::{FileInput, ProcessFiles};
use crate::standard::pool::Pool;
use crate::standard::spikes::{Gains, RECORDED_FULL_SCALE_UV, Spikes, trimmed};
use crate::standard::tracking::{Crop, Detection, Positions};
use crate::standard::widgets::{require_input, setting, settings_grid};
use ephys_rs::engine::{
    Category, CommandContext, CommandSource, DataStreamType, DevTools, EdgeTypesEnum, Flow, Node, NodeId, NodeInfo,
    NodeThread, PASS_DURATION, PassContext, PrepareContext as Context, UiContext,
};

ephys_rs::ports! {
    // The writer takes all its inputs: each pass swaps the inputs' buffers into a chunk for the writer's thread, which
    // writes the data out. If another node needs an input's data after the writer, the plan gives the writer a copy.
    // The streams on these inputs are fixed when the run starts.
    spikes: DataStreamIn<Spikes> take,
    eeg: DataStreamIn<Continuous> take,
    positions: DataStreamIn<Positions> take,
    events: DataStreamIn<Events> take,
    pos_ttl: DataStreamIn<Events> = "pos ttl" take,
    log: DataStreamIn<LogLines> take,
    /// The Acquisition Board's times, from which the writer tells when recording started by the board's clock (see
    /// `crate::standard::clock::Time`).
    time: DataStreamIn<Time> take,
    /// Settings that the writer records alongside the data. The writer only reads them.
    detection: ValIn<Detection>,
    gains: ValIn<Gains>,
    eeg_selection: ValIn<ChannelSelection> = "eeg selection",
    eeg_gains: ValIn<Gains> = "eeg gains",
    /// Sends the recorded files on once they're finished (see `after_recording`). The port is on the right because
    /// whatever processes the files next comes after the writer.
    after_recording: ValInCmdOut<FileInput, (ProcessFiles,)> = "after recording" side: Right,
}

/// Ticks a second, for timestamps.
const TIMEBASE: f64 = 96000.0;
/// The EEG's rate.
const EGF_RATE: f32 = 5000.0;
/// Samples stored for each channel of a spike.
const SPIKE_SAMPLES: usize = 50;
const SPIKE_CHANNELS: usize = 4;
/// What a position that wasn't found is stored as.
const MISSING: u16 = 1023;

/// It writes each recording's files in the base directory, named as it says (see `recording_path`).
#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WriteLtx {
    /// What the files' −128 to 127 cover, of the spikes' and the EEG's µV, as they come (amplified by their gains).
    pub scales: Scales,
}

/// What the files' 8-bit samples, −128 to 127, cover: the spikes' and the EEG's µV, either side of zero, as they come
/// (amplified by their gains). Beyond, they're clipped. The `.set` file gives each channel's gain from what came in to
/// what's stored, in Axona's terms, so files say what they hold whatever these are.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Scales {
    pub spikes_uv: f32,
    pub eeg_uv: f32,
}

impl Default for Scales {
    /// ±250 µV each, as ephys-rs has always recorded, and as gains' auto detect fills, to begin with.
    fn default() -> Self {
        Scales { spikes_uv: RECORDED_FULL_SCALE_UV, eeg_uv: RECORDED_FULL_SCALE_UV }
    }
}

/// While running: the writer's thread while recording, and its status for the window.
#[derive(Default)]
pub struct Recorder {
    writing: Option<Writing<Chunk>>,
}

impl ephys_rs::engine::PassState for Recorder {
    const WHY: &'static str = "The writer's thread while recording, which each pass's data goes to, in order, \
                               so the files are written in order.";
}

/// The form in which a chunk holds the writer's inputs (see `ephys_rs::engine::PortForm`). Each input is a `Slot`.
struct Held;

impl PortForm for Held {
    type In<T: DataStreamType> = Slot<T>;
    type Take<T: DataStreamType> = Slot<T>;
    type Out<T: DataStreamType> = ();
}

/// One input's place in a chunk: a buffer like the passes' own (`None` if the input isn't connected), and whether the
/// pass filled the buffer with data.
struct Slot<T: DataStreamType> {
    buffer: Option<T::Buffer>,
    given: bool,
}

impl<T: DataStreamType> Slot<T> {
    /// An empty slot, with a buffer for `stream` if the input is connected.
    fn new(stream: Option<T>) -> Self {
        Slot { buffer: stream.map(|s| s.buffer(PASS_DURATION)), given: false }
    }

    /// The input's data, or `None` if the pass gave none.
    fn get(&self) -> Option<&T::Buffer> {
        self.buffer.as_ref().filter(|_| self.given)
    }

    /// Swaps the buffer the pass took from the input (`taken`, if any) with the slot's own buffer, so the slot holds
    /// the pass's data and the pass gets an empty buffer back.
    fn swap(&mut self, taken: Option<&mut T::Buffer>) {
        self.given = false;
        if let (Some(buffer), Some(taken)) = (&mut self.buffer, taken) {
            std::mem::swap(taken, buffer);
            self.given = true;
        }
    }

    /// The memory used by the slot's buffer, in bytes.
    fn bytes(&self) -> usize {
        self.buffer.as_ref().map_or(0, T::bytes)
    }
}

/// One pass's data, for the writer's thread, in buffers like the passes' own so the two can be swapped (see `Held`).
/// The chunk also says when the recording took effect.
pub struct Chunk {
    inputs: DataStreamIn<Held>,
    effective: Option<DateTime<Local>>,
}

impl Chunk {
    /// Empty, with a buffer for each of `streams`.
    fn new(streams: DataStreamIn) -> Self {
        let inputs = DataStreamIn {
            spikes: Slot::new(streams.spikes),
            eeg: Slot::new(streams.eeg),
            positions: Slot::new(streams.positions),
            events: Slot::new(streams.events),
            pos_ttl: Slot::new(streams.pos_ttl),
            log: Slot::new(streams.log),
            time: Slot::new(streams.time),
        };
        Chunk { inputs, effective: None }
    }

    /// The memory one takes.
    fn bytes(&self) -> usize {
        let i = &self.inputs;
        let held = [i.spikes.bytes(), i.eeg.bytes(), i.positions.bytes(), i.events.bytes()];
        held.iter().sum::<usize>() + i.pos_ttl.bytes() + i.log.bytes() + i.time.bytes()
    }

    /// The writer's thread, with a pool of chunks for `streams`, writing to `files`.
    fn writing(
        base: &Path,
        streams: DataStreamIn,
        files: io::Result<LtxFiles>,
        on_finished: Option<OnFinished>,
    ) -> Writing<Chunk> {
        let (shown, bytes) = (base.display().to_string(), Chunk::new(streams).bytes());
        let make = move || Chunk::new(streams);
        Writing::new("LTX writer", shown, files.map(|f| Box::new(f) as Box<_>), make, bytes, on_finished)
    }
}

impl<E: PortEdges> Node<E> for WriteLtx {
    type Ports = Ports;
    type State = Recorder;
    type WorkerState = ();
    /// What it shows, for its window.
    type Shared = Pool<Status>;
    type UiState = ();
    type GlobalSettings = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Write LTX",
        category: Category::Recording,
        description: "Records tetrode spikes, continuous data (at 5 kHz), positions, digital inputs, a timing LED's \
                      changes and log lines in the LTX format, for legacy tetrode analysis code.",
    };

    /// Only while recording, and given something to record: otherwise there'd be nothing to write, but files all the
    /// same. So what feeds only writers runs only while recording.
    fn evaluate_tree_shake(&self, inputs: &[bool], _outputs: &[bool], recording: bool) -> TreeShake {
        TreeShake::needing_all(recording && inputs.contains(&true), inputs.len())
    }

    fn data_stream_out(&self, _: NodeId, _inputs: &DataStreamIn) -> NoPorts {
        NoPorts
    }

    fn val_out(&self) -> NoPorts {
        NoPorts
    }

    fn on_command(&mut self, _from: CommandSource, command: CmdIn, _context: &mut CommandContext<E>) {
        match command {}
    }

    fn on_dev_tools_tick<'a>(&'a self, dev: &mut DevTools<'a>) {
        let status = dev.shared::<Pool<Status>>().and_then(Pool::newest);
        let thread = NodeThread {
            name: "LTX writer",
            link: "buffer pool",
            flow: Flow::FromPasses,
            why: super::WHY_A_THREAD,
            swapped_outputs: Vec::new(),
            pool: status.as_ref().map(|s| s.pool),
        };
        let theme = dev.theme;
        dev.thread(thread, move |ui| status_ui(ui, status.as_deref(), true, theme));
    }

    /// While recording, opens the files and starts the thread to write them (carrying on with them through the
    /// recording's snapshots, as long as its inputs are the same).
    fn prepare_for_snapshot(&self, context: &PrepareContext<E, Self>) -> Prepare<Recorder, (), Self> {
        if context.same_streams() && !context.recording_changed {
            return Prepared::nothing().into();
        }
        let streams = context.data_stream_in::<DataStreamIn>();
        let writing = context.recording.map(|recording| {
            let date = recording.start_requested_date();
            let base = recording_path(recording.base_directory(), date);
            let recorded = Recorded::of(context, self.scales);
            let files = LtxFiles::open(&base, *date, streams, recorded);
            Chunk::writing(&base, streams, files, after_recording(context.cmd_sender(ValInCmdOut::after_recording)))
        });
        // As recording starts, someone should decide whether to carry on if the recorded times won't line up.
        let mut decisions = Vec::new();
        if context.recording.is_some() && context.recording_changed {
            let what = clock_mismatch(&streams).into_iter().chain(without_time(&streams));
            decisions.extend(what.map(|what| UserDecision { what, suggestion: None }));
        }
        Prepared::shared(Recorder { writing }).with_decisions(decisions).into()
    }

    /// While recording, hands the pass's data to the writer's thread, swapping the buffers it's in for a chunk's.
    fn process(
        &self,
        pass: &mut PassContext<E>,
        inputs: DataStreamIn<Buffers>,
        _: NoPorts,
        recorder: &mut Recorder,
        _: &mut (),
    ) {
        if !pass.recording() {
            recorder.writing = None; // finishes the files
        } else if recorder.writing.is_none() {
            // It only runs while recording, so prepares for it: it says if that's ever not so.
            recorder.writing = Some(Writing::failed("it wasn't prepared to record"));
        }
        if let Some(writing) = &recorder.writing
            && let Some(mut chunk) = writing.chunk()
        {
            chunk.effective = pass.snapshot().recording().and_then(|r| r.effective()).map(|e| e.date);
            let held = &mut chunk.inputs;
            held.spikes.swap(inputs.spikes);
            held.eeg.swap(inputs.eeg);
            held.positions.swap(inputs.positions);
            held.events.swap(inputs.events);
            held.pos_ttl.swap(inputs.pos_ttl);
            held.log.swap(inputs.log);
            held.time.swap(inputs.time);
            writing.send(chunk);
        }
        show_status(recorder.writing.as_ref(), pass.shared::<Pool<Status>>(), pass.sequence);
    }

    fn on_gui_tick(&mut self, ui: Option<&mut egui::Ui>, context: &mut UiContext<E>) {
        let Some(ui) = ui else { return };
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, context| {
            settings_grid(ui, |ui| {
                let scales = &mut node.scales;
                for (name, uv, what) in
                    [("Spikes' scale", &mut scales.spikes_uv, "Spike"), ("EEG's scale", &mut scales.eeg_uv, "EEG")]
                {
                    setting(ui, name, |ui| scale_ui(ui, uv, what));
                }
            });
            location_summary(ui, context.base_directory, "files");
            ui.add_space(4.0);
            let recording: Vec<String> = <DataStreamIn as DataStreamInPorts<E>>::ports()
                .iter()
                .zip(context.inputs)
                .filter_map(|(input, stream)| stream.map(|s| format!("{}: {}", input.name(), s.summary())))
                .collect();
            if recording.is_empty() {
                ui.weak("Nothing connected to record.");
            } else {
                ui.label(format!("Records {}.", recording.join(", ")));
            }
            let inputs = context.data_stream_in::<DataStreamIn>();
            if !context.val_in_connected(ValIn::detection) && inputs.positions.is_some() {
                ui.weak("Connect the detection to record the crop and calibration with the positions.");
            }
            if !context.val_in_connected(ValIn::gains) && inputs.spikes.is_some() {
                ui.weak("Connect the Spike Gain's gains, to record them with the spikes.");
            }
            if inputs.time.is_none() && !board_timed(&inputs).is_empty() {
                ui.weak(
                    "Connect the Acquisition Board's time output, so spikes, EEG and digital inputs are timed from the \
                     moment recording started.",
                );
            }
            let eeg = [context.val_in_connected(ValIn::eeg_selection), context.val_in_connected(ValIn::eeg_gains)];
            if eeg.contains(&false) && inputs.eeg.is_some() {
                ui.weak("Connect the Channel Selector's selection and the Continuous Gain's gains, to record them with the EEG.");
            }
            ui.separator();
            let pool = context.shared::<Pool<Status>>();
            status_ui(ui, pool.and_then(Pool::newest).as_deref(), context.running, context.theme);
        });
    }

    fn warnings(&self, inputs: &DataStreamIn) -> Vec<String> {
        let mut warnings = require_input(inputs.any_connected(), "something to record");
        if let Some(spikes) = inputs.spikes
            && spikes.group_size != SPIKE_CHANNELS as u32
        {
            warnings.push(format!(
                "LTX stores spikes as tetrodes (groups of 4), but these come in groups of {}",
                spikes.group_size
            ));
        }
        if let Some(continuous) = inputs.eeg
            && continuous.sample_rate() != EGF_RATE
        {
            warnings.push(format!("LTX stores EEG at {} kHz: add a Downsample before this input", EGF_RATE / 1000.0));
        }
        if inputs.pos_ttl.is_some() && inputs.positions.is_none() {
            warnings.push("Connect the positions too: the pos ttl times only make sense next to them".into());
        }
        warnings.extend(
            clock_mismatch(inputs).into_iter().map(|what| format!("{}{}", what[..1].to_uppercase(), &what[1..])),
        );
        warnings
    }
}

/// Some of Write LTX's inputs should be timed by the board's clock: the time, spikes, EEG and digital inputs. This lists
/// those that are connected in `inputs`, each by name, with the clock the input is actually timed by.
fn board_timed(inputs: &DataStreamIn) -> Vec<(&'static str, ClockId)> {
    let mut timed = Vec::new();
    timed.extend(inputs.time.map(|time| ("time", time.clock.id)));
    timed.extend(inputs.spikes.map(|spikes| ("spikes", spikes.clock.id)));
    timed.extend(inputs.eeg.map(|eeg| ("EEG", eeg.clock.id)));
    timed.extend(inputs.events.map(|events| ("digital inputs", events.clock.id)));
    timed
}

/// The positions, and what's timed alongside them, should share a clock: the camera's or the VR computer's. This lists
/// those that are connected in `inputs`, each by name, with the clock the input is actually timed by.
fn position_timed(inputs: &DataStreamIn) -> Vec<(&'static str, ClockId)> {
    let mut timed = Vec::new();
    timed.extend(inputs.positions.map(|positions| ("positions", positions.clock.id)));
    timed.extend(inputs.pos_ttl.map(|events| ("pos ttl", events.clock.id)));
    timed.extend(inputs.log.map(|log| ("log lines", log.clock.id)));
    timed
}

/// Each group of inputs that should share a clock (see `board_timed` and `position_timed`) can't line up if its inputs
/// come from different clocks. This says so for each group that does, for a warning or a decision.
fn clock_mismatch(inputs: &DataStreamIn) -> Vec<String> {
    let groups = [(board_timed(inputs), "board"), (position_timed(inputs), "camera or VR computer")];
    let mixed = groups.into_iter().filter(|(timed, _)| timed.windows(2).any(|pair| pair[0].1 != pair[1].1));
    mixed
        .map(|(timed, source)| {
            let names: Vec<&str> = timed.iter().map(|&(name, _)| name).collect();
            format!(
                "the {} don't all come from the same {source}, so their times won't line up in the files",
                names.join(", ")
            )
        })
        .collect()
}

/// Without the board's time, recording's start by the board's clock is only a guess from the data. If spikes, EEG or
/// digital inputs are connected but the time isn't, this says what that means, for a decision.
fn without_time(inputs: &DataStreamIn) -> Option<String> {
    (inputs.time.is_none() && !board_timed(inputs).is_empty()).then(|| {
        "Write LTX's time input isn't connected, so spikes, EEG and digital inputs will be timed from the earliest \
         of them recorded, which may be slightly after recording started"
            .into()
    })
}

/// A scale: µV either side of zero, and what it's for.
fn scale_ui(ui: &mut egui::Ui, uv: &mut f32, what: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let speed = *uv * 0.01;
        let field = egui::DragValue::new(uv)
            .range(1.0..=1_000_000.0)
            .speed(speed)
            .custom_formatter(|v, _| trimmed(v))
            .prefix("±")
            .suffix(" µV");
        ui.add(field);
        ephys_rs::gui::widgets::info_icon(
            ui,
            &format!(
                "{what} data is stored as 8-bit signed integers (−128 to 127). The voltage range set here is how \
                 the voltages coming in are mapped into it (they're clamped to fit)."
            ),
        );
    })
    .response
}

/// The settings recorded with the data, from the controls.
#[derive(Clone, Default)]
struct Recorded {
    detection: Option<Detection>,
    gains: Gains,
    /// Which channels the EEG's are, and their gains.
    eeg_selection: Option<ChannelSelection>,
    eeg_gains: Gains,
    /// What the samples cover (Write LTX's own setting).
    scales: Scales,
}

impl Recorded {
    /// The values the writer's `ValIn` ports read when the snapshot being prepared was made, with the writer's own
    /// `scales`.
    fn of<E: PortEdges>(context: &Context<E, WriteLtx>, scales: Scales) -> Self {
        Recorded {
            detection: context.val_in(ValIn::detection).cloned(),
            gains: context.val_in(ValIn::gains).cloned().unwrap_or_default(),
            eeg_selection: context.val_in(ValIn::eeg_selection).cloned(),
            eeg_gains: context.val_in(ValIn::eeg_gains).cloned().unwrap_or_default(),
            scales,
        }
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

/// The Axona format's full scale, in µV, as its `.set` files give it (`ADC_fullscale_mv`): a channel's samples cover
/// ±`AXONA_FULL_SCALE_UV / gain` of what came in.
const AXONA_FULL_SCALE_UV: f32 = 1_500_000.0;

/// A gain, recorded over `full_scale_uv` (see `Scales`), as the Axona format has it, to a tenth: the gain that fits the
/// same over its full scale (×1 over ±250 µV is 6000). Its size: an inverted channel's data is already upside down.
fn axona_gain(gain: f32, full_scale_uv: f32) -> f32 {
    (gain.abs() * AXONA_FULL_SCALE_UV / full_scale_uv * 10.0).round() / 10.0
}

/// Amplified µV as a signed byte, ±128 being ±`full_scale_uv`, rounded (half away from zero), saturating.
///
/// Rounded without `f32::round`, which is a call into the C library on x86 without SSE4.1 (as Windows builds are), a
/// value at a time: the whole part, then a step either way if what's left is a half or more. Clamped first, so both
/// are exact: it's the same as `round` for every `f32` (checked for all of them), and vectorises everywhere.
fn to_i8(uv: f32, full_scale_uv: f32) -> u8 {
    let x = (uv * 128.0 / full_scale_uv).clamp(-129.0, 128.0);
    let whole = x as i32;
    let rest = x - whole as f32;
    (whole + (rest >= 0.5) as i32 - (rest <= -0.5) as i32).clamp(-128, 127) as i8 as u8
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

/// When recording was asked for and took effect, in the `.set` file.
const PRECISE_TIME: &str = "%Y-%m-%d %H:%M:%S%.3f";

/// Room for a value filled in at the end.
const PLACEHOLDER: &str = "              ";

impl LtxFile {
    fn create(path: &Path, start: &DateTime<Local>) -> io::Result<Self> {
        let mut file = BufWriter::new(File::create(path)?);
        write!(file, "trial_date {}", start.format("%A, %d %b %Y"))?;
        write!(file, "\r\ntrial_time {}", start.format("%H:%M"))?;
        write!(file, "\r\ncreated_by {}", ephys_rs::full_version())?;
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
    /// When recording was asked for, and when it took effect (its first pass), once a chunk's said.
    start: DateTime<Local>,
    effective: Option<DateTime<Local>>,
    streams: DataStreamIn,
    set: LtxFile,
    tetrodes: Vec<Tetrode>,
    egf: Vec<Egf>,
    /// When recording started by each clock: times by that clock count from this.
    origins: Origins,
    /// The EEG sample number (see `ContinuousBuffer::start`) that goes first in the `.egf` files, once known.
    egf_first: Option<i64>,
    pos: Option<Pos>,
    ttl: Option<LtxFile>,
    pos_ttl: Option<LtxFile>,
    log: Option<BufWriter<File>>,
    log_path: PathBuf,
    /// For each record of `bytes`.
    record: Vec<u8>,
    /// What the samples cover.
    scales: Scales,
}

impl LtxFiles {
    fn open(base: &Path, start: DateTime<Local>, streams: DataStreamIn, recorded: Recorded) -> io::Result<Self> {
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
        // The full scale the samples cover: the spikes' gains', which the EEG's are taken relative to (the format has one).
        let Scales { spikes_uv: full_scale_uv, eeg_uv: eeg_full_scale_uv } = recorded.scales;
        if streams.spikes.is_some() || streams.eeg.is_some_and(|c| c.sample_rate() == EGF_RATE) {
            set.header("ADC_fullscale_mv", AXONA_FULL_SCALE_UV / 1000.0)?;
        }
        if let Some(spikes) = streams.spikes {
            for channel in 0..spikes.channels as usize {
                set.header(&format!("gain_ch_{channel}"), axona_gain(recorded.gains.get(channel), full_scale_uv))?;
            }
            for group in 0..spikes.groups() {
                let mut f = file(&format!(".{}", group + 1))?;
                f.header("num_chans", SPIKE_CHANNELS)?;
                f.header("bytes_per_timestamp", 4)?;
                f.header("samples_per_spike", SPIKE_SAMPLES)?;
                f.header("bytes_per_sample", 1)?;
                f.header("spike_format", "t,ch1,t,ch2,t,ch3,t,ch4")?;
                f.header("sample_rate", format!("{:.6} hz", spikes.sample_rate()))?;
                f.header("timebase", format!("{TIMEBASE} hz"))?;
                f.count_header("num_spikes")?;
                tetrodes.push(Tetrode { file: f, spikes: 0 });
            }
        }

        let mut egf = Vec::new();
        if let Some(continuous) = streams.eeg.filter(|c| c.sample_rate() == EGF_RATE) {
            let numbers = recorded.eeg_channels(continuous.channels);
            for channel in 0..continuous.channels {
                let number = numbers[channel as usize];
                let gain = axona_gain(recorded.eeg_gains.get(channel as usize), eeg_full_scale_uv);
                set.header(&format!("EEG_ch_{}", channel + 1), number)?;
                set.header(&format!("EEG_gain_{}", channel + 1), gain)?;
                let mut f = file(&if channel == 0 { ".egf".into() } else { format!(".egf{}", channel + 1) })?;
                f.header("num_chans", 1)?;
                f.header("sample_rate", format!("{EGF_RATE} hz"))?;
                f.header("stream_name", "eeg")?;
                f.header("chan_name", format!("CH{number}"))?;
                f.header("gain", gain)?;
                let range = eeg_full_scale_uv / recorded.eeg_gains.get(channel as usize).abs().max(f32::MIN_POSITIVE);
                f.header("range_uv", format!("{range:.1}"))?;
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
        let pos_ttl = match streams.pos_ttl {
            Some(_) if timing_led.is_none_or(|led| led.enabled) => {
                // The timing LED's changes are timed by the positions' clock (see `position_timed`), so they count
                // from the first position.
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
            effective: None,
            streams,
            set,
            tetrodes,
            egf,
            origins: Origins::default(),
            egf_first: None,
            pos,
            ttl,
            pos_ttl,
            log,
            log_path,
            record: Vec::new(),
            scales: recorded.scales,
        })
    }
}

impl LtxFiles {
    /// Sets when recording started by the board's clock (or any other clock that spikes, the EEG or digital inputs are
    /// timed by), from the first chunk with any of them: at the start of the board's time, if the chunk has it, or else
    /// at the earliest of the data.
    fn start_clocks(&mut self, chunk: &Chunk) {
        let (streams, inputs) = (&self.streams, &chunk.inputs);
        if let (Some(time), Some(buffer)) = (streams.time, inputs.time.get()) {
            self.origins.start([(time.clock.id, time.clock.seconds(buffer.start))]);
        }
        let mut starts = Vec::new();
        if let (Some(spikes), Some(buffer)) = (streams.spikes, inputs.spikes.get()) {
            // Spikes start with the data they were found in, unless a spike from the pass before is earlier still.
            let first = buffer.spikes.iter().map(|spike| spike.sample).fold(buffer.start, u64::min);
            starts.push((spikes.clock.id, spikes.clock.seconds(first as f64)));
        }
        if let (Some(eeg), Some(buffer)) = (streams.eeg, inputs.eeg.get()) {
            starts.push((eeg.clock.id, eeg.clock.seconds(buffer.start as f64)));
        }
        if let (Some(events), Some(buffer)) = (streams.events, inputs.events.get()) {
            starts.push((events.clock.id, events.clock.seconds(buffer.start)));
        }
        self.origins.start(starts);
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
    f.header("data_source", ephys_rs::full_version())?;
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
        self.effective = self.effective.or(chunk.effective);
        self.start_clocks(chunk);
        let mut bytes = 0;
        if let (Some(spikes), Some(buffer)) = (self.streams.spikes, chunk.inputs.spikes.get()) {
            let (k, samples) = (spikes.group_size as usize, spikes.waveform_samples() as usize);
            for (i, spike) in buffer.spikes.iter().enumerate() {
                let Some(tetrode) = self.tetrodes.get_mut(spike.group as usize) else { continue };
                let seconds = self.origins.since(spikes.clock, spike.sample as f64);
                let waveform = &buffer.waveforms[i * k * samples..(i + 1) * k * samples];
                self.record.clear();
                for channel in 0..SPIKE_CHANNELS {
                    self.record.extend_from_slice(&ticks(seconds));
                    let values = waveform.get(channel * samples..(channel + 1) * samples).unwrap_or(&[]);
                    // Amplified µV (by a Spike Gain), over what they cover.
                    let start = self.record.len();
                    let scale = self.scales.spikes_uv;
                    self.record.extend(values.iter().take(SPIKE_SAMPLES).map(|&v| to_i8(v, scale)));
                    self.record.resize(start + SPIKE_SAMPLES, 0);
                }
                tetrode.file.data(&self.record)?;
                tetrode.spikes += 1;
                bytes += self.record.len() as u64;
            }
        }

        if let (Some(eeg), Some(continuous)) =
            (self.streams.eeg, chunk.inputs.eeg.get().filter(|_| !self.egf.is_empty()))
            && continuous.len > 0
        {
            // Each EEG sample goes where its time says, the files' first sample being at the start of recording.
            let clock = eeg.clock;
            let first = *self.egf_first.get_or_insert_with(|| {
                let start = self.origins.of(clock.id).unwrap_or(clock.seconds(continuous.start as f64));
                (start * clock.per_second).round() as i64
            });
            let written = self.egf.first().map_or(0, |egf| egf.samples) as i64;
            let at = continuous.start as i64 - first;
            // Samples whose place is already written (or before the start) are skipped, and a gap is filled with zeros.
            let (skip, gap) = ((written - at).clamp(0, continuous.len as i64) as usize, (at - written).max(0) as usize);
            if gap > 0 {
                log::warn!("Write LTX: {gap} EEG samples missing; filled with zeros");
            }
            for (channel, egf) in self.egf.iter_mut().enumerate() {
                let values = &continuous.channel(channel)[skip..];
                self.record.clear();
                self.record.resize(gap, 0);
                // Amplified µV (by a Continuous Gain), over the full scale.
                self.record.extend(values.iter().map(|&v| to_i8(v, self.scales.eeg_uv)));
                if self.record.is_empty() {
                    continue;
                }
                egf.file.data(&self.record)?;
                egf.samples += self.record.len() as u64;
                bytes += self.record.len() as u64;
            }
        }

        if let (Some(pos), Some(positions)) = (&mut self.pos, chunk.inputs.positions.get()) {
            let clock = self.streams.positions.map(|p| p.clock);
            for sample in &positions.samples {
                let seconds = clock.map_or(0.0, |clock| self.origins.since(clock, sample.time));
                self.record.clear();
                self.record.extend_from_slice(&ticks(seconds));
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

        if let (Some(ttl), Some(events), Some(buffer)) = (&mut self.ttl, self.streams.events, chunk.inputs.events.get())
        {
            for event in &buffer.events {
                // An event the board's time puts before recording started is written at the start instead.
                let seconds = self.origins.since(events.clock, event.at).max(0.0);
                let line = format!("ttl_{} {seconds} {}\r\n", event.line + 1, event.high as u8);
                ttl.data(line.as_bytes())?;
                bytes += line.len() as u64;
            }
        }

        let position_events = (&mut self.pos_ttl, self.streams.pos_ttl, chunk.inputs.pos_ttl.get());
        if let (Some(ttl), Some(events), Some(buffer)) = position_events {
            for event in &buffer.events {
                // A timing LED's change counts from the first position, by the positions' clock: the camera's or the
                // VR computer's.
                let seconds = self.origins.since(events.clock, event.at).max(0.0);
                let line = format!("ttl_{} {seconds:.6} {}\r\n", event.line + 1, event.high as u8);
                ttl.data(line.as_bytes())?;
                bytes += line.len() as u64;
            }
        }

        if let (Some(log), Some(stream), Some(lines)) = (&mut self.log, self.streams.log, chunk.inputs.log.get()) {
            for (time, line) in lines.lines() {
                let seconds = self.origins.since(stream.clock, time);
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

    fn finish(mut self: Box<Self>) -> io::Result<Vec<PathBuf>> {
        // From the first pass recorded, as its timestamps are.
        let since = self.effective.unwrap_or(self.start);
        let seconds = (Local::now() - since).num_seconds().max(0) as u64;
        self.set.header("recording_requested", self.start.format(PRECISE_TIME))?;
        if let Some(effective) = self.effective {
            self.set.header("recording_effective", effective.format(PRECISE_TIME))?;
        }
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
    use crate::Edges;
    use crate::standard::clock::{Clock, ClockId};
    use crate::standard::ephys::Event;
    use crate::standard::spikes::Spike;
    use crate::standard::tracking::PositionSample;

    impl<T: DataStreamType> Slot<T> {
        /// The input's buffer, as given data by a pass.
        fn give(&mut self) -> &mut T::Buffer {
            self.given = true;
            self.buffer.as_mut().expect("connected")
        }
    }
    use crate::standard::clock::TimeBuffer;
    use crate::standard::tracking::pipeline::Blob;
    use crate::standard::tracking::{CAMERA_TRACKER, TimingLed};

    /// The tests time their data by two clocks: a board's, and a camera's.
    const BOARD: ClockId = ClockId::new(NodeId(1), 0);
    const CAMERA: ClockId = ClockId::new(NodeId(2), 0);

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
    fn writes_ups_and_downs_saying_which_is_which_and_the_timing_led_by_the_positions_clock() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-ltx-ttl-{}", std::process::id()));
        let base = dir.join("test");
        let (board, camera) = (Clock::new(BOARD, 1000.0), Clock::seconds_of(CAMERA));
        let events = Events { lines: 8, clock: board };
        let streams = DataStreamIn {
            positions: Some(Positions { fps: 50.0, width: 640, height: 480, source: CAMERA_TRACKER, clock: camera }),
            events: Some(events),
            pos_ttl: Some(Events { lines: 1, clock: camera }),
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
        let event = |at, line, high| Event { at, line, high };
        // Positions start at 100 s by the camera's clock; the LED comes on 0.25 s later, and goes off 0.3 s after that.
        let mut chunk = Chunk::new(streams);
        let positions = chunk.inputs.positions.give();
        positions.samples.push(sample(100.0));
        let events = chunk.inputs.events.give();
        // Recorded from the board's sample 5000.
        (events.start, events.end) = (5000.0, 5020.0);
        events.events.extend([event(5010.0, 1, true), event(5015.0, 1, false)]);
        files.write(&chunk).unwrap();
        chunk.inputs.events.given = false;
        let positions = chunk.inputs.positions.give();
        positions.samples[0] = sample(100.25);
        let events = chunk.inputs.pos_ttl.give();
        events.events.extend([event(100.25, 0, true), event(100.55, 0, false)]);
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
        let vr = Clock::seconds_of(CAMERA);
        let streams = DataStreamIn {
            positions: Some(Positions { fps: 50.0, width: 1000, height: 1000, source: "vr_computer", clock: vr }),
            pos_ttl: Some(Events { lines: 1, clock: vr }),
            log: Some(LogLines { width: 32, clock: vr }),
            ..Default::default()
        };
        let custom = [("maze.arm length", "3"), ("note", "two\r\nlines")].map(|(k, v)| (k.to_string(), v.to_string()));
        let crop = Crop::Rectangle { center: [100.0, 50.0], width: 200.0, height: 100.0, angle: 0.0 };
        let timing_led = TimingLed { enabled: true, radius: 0.0, ..Default::default() };
        let detection = Detection { crop, timing_led, custom: custom.into(), ..Default::default() };
        let recorded = Recorded { detection: Some(detection.clone()), ..Default::default() };
        let mut files = Box::new(LtxFiles::open(&base, Local::now(), streams, recorded).unwrap());
        let mut chunk = Chunk::new(streams);
        let positions = chunk.inputs.positions.give();
        positions.samples.push(PositionSample {
            frame: 0,
            time: 1.5,
            leds: [None, None],
            timing_led: None,
            pixels_per_metre: f32::NAN,
        });
        let events = chunk.inputs.pos_ttl.give();
        events.events.push(Event { at: 2.0, line: 0, high: true });
        let log = chunk.inputs.log.give();
        assert!(log.push(1.75, "first line") && log.push(4.0, "second"));
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

    /// Its window: where it writes, and its scales, each saying what it's for.
    #[test]
    fn settings_window() {
        use crate::standard::themes::THEMES;
        use crate::{NodeType, Nodes};
        use egui_kittest::kittest::Queryable as _;
        use ephys_rs::engine::{Graph, NodesEnum};
        let mut graph = Graph::<Nodes>::default();
        let id = graph.add(Nodes::create(NodeType::WriteLtx), [0.0, 0.0]);
        let inputs = DataStreamInPorts::<Edges>::streams(&DataStreamIn::default());
        let mut harness = egui_kittest::Harness::builder().with_size([460.0, 300.0]).wgpu().build_ui_state(
            |ui, graph: &mut Graph<Nodes>| {
                graph.edit(id, &inputs, &THEMES[0].colours, None, None, None, false, false, None, |node, context| {
                    node.on_gui_tick(Some(ui), context)
                });
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        assert!(harness.query_by_label_contains("8-bit signed integers").is_none());
        harness.get_all_by_label("What's this?").next().unwrap().hover();
        harness.run_steps(4);
        harness.get_by_label_contains("8-bit signed integers");
    }

    /// Bytes are rounded as `f32::round` rounds (half away from zero), about every half and either side of it.
    #[test]
    fn bytes_round_halves_away_from_zero() {
        for half in (-260..=260).map(|h| h as f32 / 2.0) {
            for uv in [half.next_down(), half, half.next_up()] {
                let rounded = (uv.round() as i32).clamp(-128, 127) as i8;
                assert_eq!(to_i8(uv, 128.0) as i8, rounded, "{uv:e}");
            }
        }
    }

    /// Its scales map what comes in to the files' 8 bits, and the gains it gives in the `.set` file follow: twice the
    /// scale stores half as much, and is half the gain, from what came in to what's stored.
    #[test]
    fn its_scales_map_microvolts_to_the_files_bytes() {
        assert_eq!(to_i8(125.0, 250.0) as i8, 64);
        assert_eq!(to_i8(125.0, 500.0) as i8, 32);
        assert_eq!((to_i8(600.0, 500.0) as i8, to_i8(-600.0, 500.0) as i8), (127, -128), "beyond, clipped");
        assert_eq!((to_i8(f32::NAN, 250.0), to_i8(f32::INFINITY, 250.0) as i8), (0, 127));
        assert_eq!((axona_gain(1.0, 250.0), axona_gain(1.0, 500.0), axona_gain(-2.0, 500.0)), (6000.0, 3000.0, 6000.0));
    }

    #[test]
    fn writes_spikes_eeg_and_positions() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-ltx-{}", std::process::id()));
        let base = dir.join("test");
        let spikes = Spikes { channels: 4, group_size: 4, pre: 8, post: 32, clock: Clock::new(BOARD, 30000.0) };
        let camera = Clock::seconds_of(CAMERA);
        let streams = DataStreamIn {
            spikes: Some(spikes),
            eeg: Some(Continuous { channels: 2, clock: Clock::new(BOARD, EGF_RATE as f64) }),
            positions: Some(Positions { fps: 50.0, width: 640, height: 480, source: CAMERA_TRACKER, clock: camera }),
            ..Default::default()
        };
        // Gains recorded: a µV a step on the first channel, half a µV on the second.
        let mut gains = Gains::default();
        let channels = crate::standard::spikes::Channels { count: 4, group_size: 4 };
        gains.set_range(0, 128.0, channels);
        gains.set_range(1, 64.0, channels);
        // The EEG: channels 5 and 7 of the board's, at ±200 µV.
        let mut eeg_gains = Gains { scope: crate::standard::spikes::GainScope::All, ..Default::default() };
        eeg_gains.set_range(0, 200.0, crate::standard::spikes::Channels { count: 2, group_size: 1 });
        let eeg_selection = Some(ChannelSelection { apply: true, channels: "5, 7".into() });
        let recorded = Recorded { detection: None, gains, eeg_selection, eeg_gains, scales: Scales::default() };
        let requested = Local::now();
        let mut files = Box::new(LtxFiles::open(&base, requested, streams, recorded).unwrap());

        // Recorded from sample 1000, a spike at sample 1900: 900 samples in, 30 ms, 2880 ticks.
        let mut chunk = Chunk::new(streams);
        let effective = requested + chrono::Duration::milliseconds(40);
        chunk.effective = Some(effective);
        let buffer = chunk.inputs.spikes.give();
        buffer.start = 1000;
        files.write(&chunk).unwrap();
        let buffer = chunk.inputs.spikes.give();
        buffer.start = 1600;
        buffer.spikes.push(Spike { sample: 1900, group: 0, channel: 0 });
        // Amplified µV (by a Spike Gain): what are, over the scale, int8 values from -80.
        let full_scale = Scales::default().spikes_uv;
        let step = full_scale / 128.0;
        buffer.waveforms = (0..160).map(|i| (i as f32 - 80.0) * step).collect();
        // The EEG has two samples on each channel (at 5 kHz), from the start of recording (sample 1000 at 30 kHz), and
        // amplified as the spikes are. One sample is beyond the full scale, so is clipped.
        let continuous = chunk.inputs.eeg.give();
        (continuous.start, continuous.len) = (167, 2);
        continuous.channel_mut(0).copy_from_slice(&[0.0, 30.0 * step]);
        continuous.channel_mut(1).copy_from_slice(&[60.0 * step, 2.0 * full_scale]);
        let blob = Blob { x: 100.0, y: 50.0, area: 30 };
        let positions = chunk.inputs.positions.give();
        positions.samples.push(PositionSample {
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
        assert_eq!(header(&tetrode, "created_by"), ephys_rs::full_version(), "which build wrote it");
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
        assert_eq!(data(&egf).iter().map(|b| *b as i8).collect::<Vec<_>>(), vec![60, 127]);
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
        let (asked, took) = (header(&set, "recording_requested"), header(&set, "recording_effective"));
        assert_eq!(asked, requested.format(PRECISE_TIME).to_string(), "to the millisecond");
        assert_eq!(took, effective.format(PRECISE_TIME).to_string());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The first pass recorded can have different stretches of the spikes, EEG and digital inputs. Whatever it has,
    /// all three count from when recording started by the board's time, and the EEG's samples are placed by their
    /// times.
    #[test]
    fn times_by_the_board_count_from_its_time() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-ltx-time-{}", std::process::id()));
        let base = dir.join("test");
        let board = Clock::new(BOARD, 30000.0);
        let streams = DataStreamIn {
            spikes: Some(Spikes { channels: 4, group_size: 4, pre: 8, post: 32, clock: Clock::new(BOARD, 30000.0) }),
            eeg: Some(Continuous { channels: 1, clock: Clock::new(BOARD, EGF_RATE as f64) }),
            events: Some(Events { lines: 8, clock: board }),
            time: Some(Time { clock: board }),
            ..Default::default()
        };
        assert_eq!((clock_mismatch(&streams), without_time(&streams)), (vec![], None));
        let mut files = Box::new(LtxFiles::open(&base, Local::now(), streams, Recorded::default()).unwrap());

        // Recording starts at the board's sample 3000 (sample 500 at 5 kHz). The spikes were found in data from
        // earlier, but still count from 3000: a spike at 3150 is 5 ms in, which is 480 ticks.
        let mut chunk = Chunk::new(streams);
        *chunk.inputs.time.give() = TimeBuffer { start: 3000.0, end: 3600.0 };
        let buffer = chunk.inputs.spikes.give();
        buffer.start = 2700;
        buffer.spikes.push(Spike { sample: 3150, group: 0, channel: 0 });
        buffer.waveforms = vec![0.0; 160];
        let events = chunk.inputs.events.give();
        (events.start, events.end) = (3000.0, 3600.0);
        events.events.push(Event { at: 3300.0, line: 0, high: true });
        // The EEG starts two samples before recording started, so those two are skipped.
        let eeg = chunk.inputs.eeg.give();
        (eeg.start, eeg.len) = (498, 4);
        eeg.channel_mut(0).copy_from_slice(&[1.0, 2.0, 3.0, 4.0].map(|n| n * full_scale_step()));
        files.write(&chunk).unwrap();
        // The next EEG buffer comes after a gap of 3 samples, which is filled with zeros.
        let mut chunk = Chunk::new(streams);
        let eeg = chunk.inputs.eeg.give();
        (eeg.start, eeg.len) = (505, 1);
        eeg.channel_mut(0)[0] = 5.0 * full_scale_step();
        files.write(&chunk).unwrap();
        files.finish().unwrap();

        let tetrode = read(&base, ".1");
        assert_eq!(i32::from_be_bytes(data(&tetrode)[..4].try_into().unwrap()), 480);
        assert_eq!(data(&read(&base, ".ttl")), b"ttl_1 0.01 1\r\n");
        let egf = read(&base, ".egf");
        assert_eq!(data(&egf).iter().map(|b| *b as i8).collect::<Vec<_>>(), vec![3, 4, 0, 0, 0, 5]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Amplified µV in a step of the files' bytes.
    fn full_scale_step() -> f32 {
        Scales::default().spikes_uv / 128.0
    }

    /// If spikes, the EEG and digital inputs aren't all timed by one clock, or the board's time isn't connected, the
    /// recorded times won't line up, which someone should decide about.
    #[test]
    fn says_if_times_wont_line_up() {
        let (board, other) = (Clock::new(BOARD, 30000.0), Clock::new(CAMERA, 30000.0));
        let spikes = Spikes { channels: 4, group_size: 4, pre: 8, post: 32, clock: Clock::new(BOARD, 30000.0) };
        let mismatched = DataStreamIn {
            spikes: Some(spikes),
            events: Some(Events { lines: 8, clock: other }),
            time: Some(Time { clock: board }),
            ..Default::default()
        };
        let mismatch = clock_mismatch(&mismatched);
        assert!(mismatch.len() == 1 && mismatch[0].contains("time, spikes, digital inputs"), "{mismatch:?}");
        assert!(
            Node::<Edges>::warnings(&WriteLtx::default(), &mismatched)
                .iter()
                .any(|w| w.starts_with("The time, spikes"))
        );
        let untimed = DataStreamIn { spikes: Some(spikes), ..Default::default() };
        assert!(without_time(&untimed).is_some());
        assert_eq!(without_time(&DataStreamIn::default()), None, "nothing by the board's clock");
    }

    #[test]
    fn says_if_positions_times_wont_line_up() {
        let (board, camera, vr) =
            (Clock::new(BOARD, 1000.0), Clock::seconds_of(CAMERA), Clock::seconds_of(ClockId::new(NodeId(3), 0)));
        let positions = Positions { fps: 50.0, width: 640, height: 480, source: CAMERA_TRACKER, clock: camera };
        let mixed = DataStreamIn {
            positions: Some(positions),
            pos_ttl: Some(Events { lines: 1, clock: vr }),
            log: Some(LogLines { width: 80, clock: vr }),
            ..Default::default()
        };
        let mismatch = clock_mismatch(&mixed);
        assert!(mismatch.len() == 1 && mismatch[0].contains("positions, pos ttl, log lines"), "{mismatch:?}");
        let by_board = DataStreamIn {
            positions: Some(positions),
            pos_ttl: Some(Events { lines: 1, clock: board }),
            events: Some(Events { lines: 8, clock: board }),
            ..Default::default()
        };
        let mismatch = clock_mismatch(&by_board);
        assert!(mismatch.len() == 1 && mismatch[0].contains("positions, pos ttl"), "{mismatch:?}");
        let together = DataStreamIn { pos_ttl: Some(Events { lines: 1, clock: camera }), ..by_board };
        assert!(clock_mismatch(&together).is_empty());
    }
}
