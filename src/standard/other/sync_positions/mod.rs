//! Sync Positions: puts a recording's positions on the board's clock, after it's recorded, using the timing LED's
//! pulses, which both the camera (`name.pos.ttl`) and one of the board's digital inputs (`name.ttl`) saw. It's given the
//! recording's `.set` file (picked here, or sent with `RunOnFiles`, by Write LTX once it's finished, say), and finds the
//! rest beside it (see `files`). It runs here, on a thread of its own, not in a terminal, and it's nothing to do with
//! passes.
//!
//! The positions as recorded are kept, as `name.pos.original` (or `name (2).pos.original`... if that's taken), and the
//! synced ones written in their place, saying which original they're from: synced again, they're synced from it, not
//! from themselves.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};

use crate::engine::{Category, EdgeInfo, EdgeType, Node, NodeInfo, Output, SettingPort, UiContext};
use crate::standard::other::run_command::RunOnFiles;
use crate::standard::widgets::{setting, settings_grid};
use crate::theme::Colours;
use crate::triggers::ControlTriggers;
use crate::{DataStream, Edges, Setting, Trigger};

mod algorithm;
pub mod files;

use algorithm::Stats;
use files::{LtxFile, Plan, Recording};

/// How it syncs, and the recording to sync by hand.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncSettings {
    /// The recording's `.set` file, to sync by hand: empty for none.
    pub set_file: PathBuf,
    /// Which of the board's digital inputs (from 1) the timing LED's pulses go to.
    pub board_line: u32,
}

impl Default for SyncSettings {
    fn default() -> Self {
        SyncSettings { set_file: PathBuf::new(), board_line: 1 }
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

/// What a run did: the recording's files, what it did to the positions there were, and what it found.
#[derive(Clone, PartialEq, Debug)]
pub struct Report {
    pub recording: Recording,
    pub plan: Plan,
    pub stats: Stats,
}

/// Syncs the recording `set` is the `.set` file of.
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
    if let Plan::Keep { original } = &plan {
        std::fs::copy(&recording.pos, original).map_err(failed(original))?;
    }
    let pos = LtxFile::read(plan.original()).map_err(failed(plan.original()))?;
    let board = LtxFile::read(&recording.ttl).map_err(failed(&recording.ttl))?;
    let led = LtxFile::read(&recording.pos_ttl).map_err(failed(&recording.pos_ttl))?;
    let (board, led) = (files::changes(&board)?, files::changes(&led)?);
    let (mut synced, stats) = algorithm::sync(&pos, &board, &led, settings)?;
    synced.set_header(files::SYNCED_FROM, file_name(plan.original()));
    synced.set_header(files::SYNCED_BY, crate::full_version());
    synced.write(&recording.pos).map_err(failed(&recording.pos))?;
    Ok(Report { recording, plan, stats })
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
        result: Result<Report, String>,
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
            let result = run(&set, &settings);
            *job.0.lock().unwrap() = Status::Done { set, result };
        });
        match thread {
            // The app waits for it before quitting, so the positions aren't left half-written.
            Ok(thread) => crate::background::finish_later(thread),
            Err(e) => *status = Status::Done { set: PathBuf::new(), result: Err(format!("Couldn't start: {e}")) },
        }
    }

    /// Says why it can't sync, unless it's syncing one already.
    fn fail(&self, problem: String) {
        let mut status = self.0.lock().unwrap();
        if !matches!(*status, Status::Running { .. }) {
            *status = Status::Done { set: PathBuf::new(), result: Err(problem) };
        }
    }

    fn status(&self) -> Status {
        self.0.lock().unwrap().clone()
    }
}

/// What a recording's `.set` file comes to: its files, whether they're there, and what syncing would do.
struct Preview {
    set: PathBuf,
    made: Instant,
    found: Option<(Recording, [bool; 3], Option<Plan>)>,
}

/// A Sync Positions' state on the GUI's side (its `UiState`): what it's syncing, or last synced, and what the
/// recording picked comes to, kept for a second, as it reads the disk.
#[derive(Default)]
pub struct SyncState {
    job: Job,
    preview: Option<Preview>,
}

impl SyncState {
    fn preview(&mut self, set: &Path) -> Option<&(Recording, [bool; 3], Option<Plan>)> {
        let fresh = self.preview.as_ref().is_some_and(|p| p.set == set && p.made.elapsed() < Duration::from_secs(1));
        if !fresh {
            let found = Recording::of(set).map(|recording| {
                let there = [&recording.pos, &recording.ttl, &recording.pos_ttl].map(|path| path.exists());
                let plan = recording.plan().ok();
                (recording, there, plan)
            });
            self.preview = Some(Preview { set: set.to_path_buf(), made: Instant::now(), found });
        }
        self.preview.as_ref()?.found.as_ref()
    }
}

/// Puts positions on the board's clock, after recording.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncPositions {
    pub settings: SyncSettings,
}

impl SyncPositions {
    fn window(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        ui.horizontal(|ui| context.node_toolbar(ui));
        ui.label(
            "Puts a recording's positions on the board's clock, by the timing LED's pulses, which both the camera \
             and the board saw.",
        );
        ui.add_space(6.0);
        let job = context.ui_state::<SyncState>().job.clone();
        let (status, locked) = (job.status(), context.locked);
        let running = matches!(status, Status::Running { .. });
        // What's connected supplies the recordings, while it is.
        let senders = context.controlled_by.first().cloned().unwrap_or_default();
        let connected = !senders.is_empty();
        let settings = &mut self.settings;
        ui.add_enabled_ui(!locked && !running, |ui| {
            settings_grid(ui, |ui| {
                let label = ui.label("Recording");
                if connected {
                    ui.weak(format!("Supplied by {}: disconnect or disable it to pick one yourself", any_of(&senders)));
                } else {
                    ui.horizontal(|ui| {
                        if ui.button("Select .set file…").clicked()
                            && let Some(set) = rfd::FileDialog::new().add_filter("LTX recording", &["set"]).pick_file()
                        {
                            settings.set_file = set;
                        }
                        let shown = if settings.set_file.as_os_str().is_empty() {
                            "none".to_string()
                        } else {
                            settings.set_file.display().to_string()
                        };
                        ui.add(egui::Label::new(&shown).truncate()).on_hover_text(shown);
                    })
                    .response
                    .labelled_by(label.id);
                }
                ui.end_row();
                setting(ui, "Board's TTL line", |ui| {
                    ui.add(egui::DragValue::new(&mut settings.board_line).range(1..=64))
                        .on_hover_text("The board's digital input the timing LED's pulses go to")
                });
            });
        });
        ui.add_space(6.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Files");
            ui.add_space(6.0);
            let picked = !self.settings.set_file.as_os_str().is_empty();
            let run = ui.add_enabled(picked && !locked && !running && !connected, egui::Button::new("Run"));
            let run = if connected {
                run.on_disabled_hover_text("It runs by itself on the recordings sent")
            } else {
                run.on_hover_text("Sync the positions of the recording picked")
            };
            // As if it were sent it: so it's the same either way.
            if run.clicked() {
                context.trigger(None, RunOnFiles(vec![self.settings.set_file.clone()]));
            }
            if connected {
                ui.weak(format!("Run triggered by {}", any_of(&senders)));
            }
        });
        let theme = context.theme;
        // The files of the recording last synced, or being synced, or else of the one picked.
        let shown = match &status {
            Status::Running { set, .. } | Status::Done { set, .. } if !set.as_os_str().is_empty() => Some(set.clone()),
            _ if !connected && !self.settings.set_file.as_os_str().is_empty() => Some(self.settings.set_file.clone()),
            _ => None,
        };
        let done = match &status {
            Status::Done { result: Ok(report), .. } => Some(report),
            _ => None,
        };
        match shown {
            Some(set) => match done {
                Some(report) => files_grid(ui, &report.recording, [true; 3], Some(&report.plan), theme),
                None => match context.ui_state::<SyncState>().preview(&set) {
                    Some((recording, there, plan)) => files_grid(ui, recording, *there, plan.as_ref(), theme),
                    None => {
                        ui.colored_label(theme.warning, "Not a .set file");
                    }
                },
            },
            None => {
                ui.weak("Each recording's .pos, .ttl and .pos.ttl files, found beside its .set file");
            }
        }
        ui.add_space(6.0);
        match &status {
            Status::Idle => {}
            Status::Running { set, started } => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(format!("Syncing {}… {:.0} s", file_name(set), started.elapsed().as_secs_f64()));
                });
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            Status::Done { result: Ok(report), .. } => outcome(ui, report, theme),
            Status::Done { result: Err(problem), .. } => {
                ui.add(egui::Label::new(RichText::new(problem).color(theme.error)).wrap());
            }
        }
    }
}

/// The recording's three files: the positions read (their original, if they were synced before), and its TTLs. Any
/// not there are said to be missing.
fn files_grid(ui: &mut egui::Ui, recording: &Recording, there: [bool; 3], plan: Option<&Plan>, theme: &Colours) {
    let (positions, note) = match plan {
        Some(Plan::Resync { original }) => {
            (original, format!("the original: {} was synced before", file_name(&recording.pos)))
        }
        _ => (&recording.pos, String::new()),
    };
    let rows = [
        ("Positions", positions, note),
        ("Board's TTLs", &recording.ttl, String::new()),
        ("Timing LED's TTLs", &recording.pos_ttl, String::new()),
    ];
    egui::Grid::new("sync files").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        for ((what, path, note), there) in rows.into_iter().zip(there) {
            ui.label(what);
            ui.horizontal(|ui| {
                if there {
                    ui.label(RichText::new(file_name(path)).monospace());
                } else {
                    ui.label(RichText::new(format!("{} · missing", file_name(path))).monospace().color(theme.warning));
                }
                if !note.is_empty() {
                    ui.weak(note);
                }
            });
            ui.end_row();
        }
    });
}

/// What a run did, and what it found.
fn outcome(ui: &mut egui::Ui, report: &Report, theme: &Colours) {
    let pos = file_name(&report.recording.pos);
    ui.label(RichText::new(format!("✔ Synced {pos}")).color(theme.success));
    ui.label(match &report.plan {
        Plan::Keep { original } => format!("The positions as recorded are kept as {}.", file_name(original)),
        Plan::Resync { original } => format!(
            "{pos} had been synced before, so it was synced again from the positions as recorded, in {}.",
            file_name(original)
        ),
    });
    ui.add_space(4.0);
    let stats = &report.stats;
    egui::Grid::new("sync stats").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
        let row = |ui: &mut egui::Ui, what: &str, value: String| {
            ui.weak(what);
            ui.label(value);
            ui.end_row();
        };
        row(ui, "Positions", format!("{} over {:.1} s", stats.positions, stats.positions_seconds));
        row(ui, "Board's TTL changes", stats.board_changes.to_string());
        row(ui, "Timing LED's changes", stats.led_changes.to_string());
    });
}

/// `names`, as a list of alternatives.
fn any_of(names: &[String]) -> String {
    match names {
        [others @ .., last] if !others.is_empty() => format!("{} or {last}", others.join(", ")),
        _ => names.concat(),
    }
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

    const WINDOW_SIZE: [f32; 2] = [620.0, 420.0];
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
    fn evaluate_tree_shake(&self, inputs: &[bool], _outputs: &[bool]) -> crate::engine::TreeShake {
        crate::engine::TreeShake::needing_all(false, inputs.len())
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
