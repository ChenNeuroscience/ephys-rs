//! Sync Positions: puts a recording's positions on the board's clock, after it's recorded, using the timing LED's
//! pulses, which both the camera (`name.pos.ttl`) and one of the board's digital inputs (`name.ttl`) saw. It's given the
//! recording's `.set` file (picked here, or sent with `RunOnFiles`, by Write LTX once it's finished, say), and finds the
//! rest beside it (see `files`). It runs here, on a thread of its own, not in a terminal, and it's nothing to do with
//! passes.
//!
//! The positions as recorded are kept, as `name.pos.original` (or `name (2).pos.original`... if that's taken), and the
//! synced ones written in their place, saying which original they're from: synced again, they're synced from it, not
//! from themselves. Nothing's written unless it syncs.
//!
//! The board's clock is taken as true. Syncing goes in steps: the timing LED's changes paired with the board's
//! (`pairs`), the offset between the clocks through the recording (`offsets`), the positions put on the board's clock at
//! the rate they say they're at (`resample`), then what's worth a warning (`checks`). Its window's in `window`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};

use crate::standard::other::run_command::RunOnFiles;
use crate::{DataStream, Edges, Setting, Trigger};
use ephys_rs::engine::{Category, EdgeInfo, EdgeType, Node, NodeInfo, Output, SettingPort, UiContext};
use ephys_rs::triggers::ControlTriggers;

mod checks;
pub mod files;
mod offsets;
mod pairs;
mod resample;
mod window;

use files::{Change, LtxFile, POS_RECORD, Plan, Recording};
use offsets::{Clock, Knot};
use pairs::Pairing;
use resample::Edits;
pub use window::SyncState;

/// How it syncs, and the recording to sync by hand. Times are in seconds.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncSettings {
    /// The recording's `.set` file, to sync by hand: empty for none.
    pub set_file: PathBuf,
    /// Which of the board's digital inputs (from 1) the timing LED's pulses go to.
    pub board_line: u32,
    /// How far apart by their clocks the same change can be, on the board and by the camera.
    pub within: f64,
    /// How far from a change any other on the other side makes pairing it ambiguous.
    pub alone: f64,
    /// How often through the recording the offset between the clocks is found.
    pub step: f64,
    /// How far either side of each step the pairs' offsets are taken the median of.
    pub around: f64,
}

impl Default for SyncSettings {
    fn default() -> Self {
        SyncSettings { set_file: PathBuf::new(), board_line: 1, within: 1.0, alone: 2.0, step: 30.0, around: 60.0 }
    }
}

impl SyncSettings {
    /// How it syncs, in words, on one line, in ASCII, as old readers expect: for the synced positions' header.
    pub fn info(&self) -> String {
        format!(
            "Sync Positions put these positions on the board's clock (as name.ttl times them) by the timing LED's \
             changes (name.pos.ttl) and the board's line {}: changes paired within +/-{} s, if no other was within +/-{} s; \
             the clocks' offset the median of the pairs within +/-{} s of every {} s, changing steadily between; \
             positions put in the nearest slot of their sample_rate from the start of recording, the nearest kept \
             where two were, and empty slots filled with positions not found",
            self.board_line, self.within, self.alone, self.around, self.step
        )
    }
}

impl EdgeType for SyncSettings {
    const INFO: EdgeInfo = EdgeInfo {
        name: "sync",
        colour: Color32::from_rgb(150, 200, 170),
        description: "How positions are put on the board's clock: what's connected to one can send it recordings to sync.",
    };

    fn summary(&self) -> String {
        format!("board line {}", self.board_line)
    }
}

/// What a run did: the recording's files, what it did to the positions there were, and what syncing found.
#[derive(Clone, PartialEq, Debug)]
pub struct Report {
    pub recording: Recording,
    pub plan: Plan,
    pub synced: Synced,
}

/// Syncs the recording `set` is the `.set` file of. Nothing's written unless it syncs.
fn run(set: &Path, settings: &SyncSettings) -> Result<Report, String> {
    let recording = Recording::of(set).ok_or_else(|| format!("Not a .set file: {}", set.display()))?;
    for path in [&recording.pos, &recording.ttl, &recording.pos_ttl] {
        if !path.exists() {
            return Err(format!("There's no {}", file_name(path)));
        }
    }
    let failed = |path: &Path| {
        let name = file_name(path);
        move |e: std::io::Error| format!("{name}: {e}")
    };
    let plan = recording.plan().map_err(failed(&recording.pos))?;
    // As recorded, they're the positions themselves, kept as the original once they're synced.
    let source = match &plan {
        Plan::Keep { .. } => &recording.pos,
        Plan::Resync { original } => original,
    };
    let pos = LtxFile::read(source).map_err(failed(source))?;
    let board = LtxFile::read(&recording.ttl).map_err(failed(&recording.ttl))?;
    let led = LtxFile::read(&recording.pos_ttl).map_err(failed(&recording.pos_ttl))?;
    let (board, led) = (files::changes(&board)?, files::changes(&led)?);
    let (mut out, synced) = sync(&pos, &board, &led, settings)?;
    if let Plan::Keep { original } = &plan {
        std::fs::copy(&recording.pos, original).map_err(failed(original))?;
    }
    out.set_header(files::SYNCED_FROM, file_name(plan.original()));
    out.set_header(files::SYNCED_BY, ephys_rs::full_version());
    out.set_header(files::POST_PROCESSING_INFO, settings.info());
    out.write(&recording.pos).map_err(failed(&recording.pos))?;
    Ok(Report { recording, plan, synced })
}

/// What syncing found and did.
#[derive(Clone, PartialEq, Debug)]
pub struct Synced {
    /// Positions read.
    pub recorded: usize,
    /// Their rate, as they say (`sample_rate`).
    pub rate: f64,
    pub pairing: Pairing,
    pub knots: Vec<Knot>,
    /// How much faster the board's clock runs than the camera's, by the pairs' best straight line, if there are pairs
    /// apart in time.
    pub drift_ppm: Option<f64>,
    /// The median distance of the pairs' offsets from the offset where they are, in seconds.
    pub scatter: f64,
    pub edits: Edits,
    pub warnings: Vec<String>,
}

/// `pos`, put on the board's clock by `board`'s changes (all its lines) and the timing LED's, `led`: step by step, the
/// changes paired (`pairs`), the offset between the clocks through the recording (`offsets`), and the positions
/// resampled on the board's clock (`resample`), then what's worth a warning (`checks`).
fn sync(pos: &LtxFile, board: &[Change], led: &[Change], settings: &SyncSettings) -> Result<(LtxFile, Synced), String> {
    if !pos.data.len().is_multiple_of(POS_RECORD) {
        return Err(format!("The positions aren't whole records of {POS_RECORD} bytes"));
    }
    let timebase = files::rate(pos, "timestamp_timebase")?;
    let rate = files::rate(pos, "sample_rate")?;
    let missing = pos.header("position_missing_coordinate").and_then(|m| m.parse().ok()).unwrap_or(1023);
    let line = settings.board_line;
    let board: Vec<Change> = board.iter().filter(|c| c.line == line).copied().collect();
    if board.is_empty() {
        return Err(format!("The board's line {line} never changed: is the timing LED on another?"));
    }
    if led.is_empty() {
        return Err("The camera never saw the timing LED change".into());
    }
    let pairing = pairs::pair(&board, led, settings.within, settings.alone);
    if pairing.pairs.is_empty() {
        let (b, l) = (pairing.board, pairing.led);
        return Err(format!(
            "None of the timing LED's {} changes paired with the board's line {line}'s {} ({} ambiguous, {} unmatched):              is it the right line?",
            l.changes(),
            b.changes(),
            l.ambiguous,
            l.unmatched
        ));
    }
    let records = pos.data.chunks_exact(POS_RECORD);
    let last = records.clone().map(files::timestamp).max().unwrap_or(0) as f64 / timebase;
    let mut offsets: Vec<f64> = pairing.pairs.iter().map(pairs::Pair::offset).collect();
    let typical = offsets::median(&mut offsets).unwrap_or(0.0);
    let knots = offsets::knots(&pairing.pairs, settings.step, settings.around, last + typical);
    let clock = Clock::new(&knots).ok_or("None of the pairs are while there were positions")?;
    let mut residuals: Vec<f64> = pairing.pairs.iter().map(|p| (p.offset() - clock.offset_at(p.board)).abs()).collect();
    let scatter = offsets::median(&mut residuals).unwrap_or(0.0);
    let (data, edits) = resample::resample(&pos.data, timebase, &clock, rate, missing);
    let mut synced = Synced {
        recorded: records.len(),
        rate,
        drift_ppm: drift_ppm(&pairing.pairs),
        pairing,
        knots,
        scatter,
        edits,
        warnings: Vec::new(),
    };
    synced.warnings = checks::warnings(&synced);
    let mut out = LtxFile { headers: pos.headers.clone(), data };
    out.set_header("num_pos_samples", edits.positions);
    Ok((out, synced))
}

/// The slope of the pairs' offsets over the board's time, by least squares, in parts per million.
fn drift_ppm(pairs: &[pairs::Pair]) -> Option<f64> {
    let n = pairs.len() as f64;
    let mean_t = pairs.iter().map(|p| p.board).sum::<f64>() / n;
    let mean_o = pairs.iter().map(|p| p.offset()).sum::<f64>() / n;
    let sxx: f64 = pairs.iter().map(|p| (p.board - mean_t).powi(2)).sum();
    let sxy: f64 = pairs.iter().map(|p| (p.board - mean_t) * (p.offset() - mean_o)).sum();
    (sxx > 0.0).then(|| sxy / sxx * 1e6)
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// What it's doing, or last did.
#[derive(Clone, PartialEq, Debug, Default)]
enum Status {
    #[default]
    Idle,
    Running {
        set: PathBuf,
        started: Instant,
    },
    Done {
        set: PathBuf,
        result: Result<Box<Report>, String>,
    },
}

/// Syncing, on a thread of its own: one recording at a time.
#[derive(Clone, Default)]
pub struct Job(Arc<Mutex<Status>>);

impl Job {
    /// Syncs `set`'s recording, unless it's syncing one already.
    fn start(&self, set: PathBuf, settings: SyncSettings) {
        let mut status = self.0.lock().unwrap();
        if matches!(*status, Status::Running { .. }) {
            return;
        }
        *status = Status::Running { set: set.clone(), started: Instant::now() };
        let job = self.clone();
        let thread = std::thread::Builder::new().name("sync positions".into()).spawn(move || {
            let result = run(&set, &settings).map(Box::new);
            // To the console, and counted in the status bar, as it may have run by itself.
            match &result {
                Ok(report) => {
                    for warning in &report.synced.warnings {
                        log::warn!("Sync Positions: {}: {warning}", file_name(&set));
                    }
                }
                Err(problem) => log::error!("Sync Positions: {}: {problem}", file_name(&set)),
            }
            *job.0.lock().unwrap() = Status::Done { set, result };
        });
        match thread {
            // The app waits for it before quitting, so the positions aren't left half-written.
            Ok(thread) => ephys_rs::background::finish_later(thread),
            Err(e) => *status = Status::Done { set: PathBuf::new(), result: Err(format!("Couldn't start: {e}")) },
        }
    }

    /// Says why it can't sync, unless it's syncing one already.
    fn fail(&self, problem: String) {
        let mut status = self.0.lock().unwrap();
        if !matches!(*status, Status::Running { .. }) {
            log::error!("Sync Positions: {problem}");
            *status = Status::Done { set: PathBuf::new(), result: Err(problem) };
        }
    }

    fn status(&self) -> Status {
        self.0.lock().unwrap().clone()
    }
}

/// Puts positions on the board's clock, after recording.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncPositions {
    pub settings: SyncSettings,
}

impl Node<Edges> for SyncPositions {
    type State = ();
    type Shared = ();
    type UiState = SyncState;
    type GlobalSettings = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Sync Positions",
        category: Category::Other,
        description: "Puts a recording's positions on the board's clock, by the timing LED's pulses: by hand, or on \
                      recordings sent to it (by Write LTX, after recording).",
    };

    const WINDOW_SIZE: [f32; 2] = [680.0, 780.0];
    const SHOW_IN_BASIC_DEFAULT: bool = true;

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "sync", value: self.settings.clone().into(), on_right: false }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Sync(settings) = value {
            self.settings = settings;
        }
    }

    /// Never in a pass: it's sent what to do (see `on_trigger`), and has nothing to do with the data going by.
    fn evaluate_tree_shake(&self, inputs: &[bool], _outputs: &[bool]) -> ephys_rs::engine::TreeShake {
        ephys_rs::engine::TreeShake::needing_all(false, inputs.len())
    }

    fn triggers(&self, _setting: usize) -> &'static [crate::TriggerKind] {
        &[crate::TriggerKind::RunOnFiles]
    }

    /// Syncs the recording whose `.set` file's among the files sent (by hand, too).
    fn on_trigger(&mut self, trigger: Trigger, context: &mut UiContext<Edges>, _triggers: &ControlTriggers<Trigger>) {
        if let Trigger::RunOnFiles(RunOnFiles(paths)) = trigger {
            let job = &context.ui_state::<SyncState>().job;
            match paths.into_iter().find(|path| path.extension().is_some_and(|e| e == "set")) {
                Some(set) => job.start(set, self.settings.clone()),
                None => job.fail("There's no .set file among the files sent".into()),
            }
        }
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        self.window(ui, context);
    }
}

#[cfg(test)]
mod tests;
