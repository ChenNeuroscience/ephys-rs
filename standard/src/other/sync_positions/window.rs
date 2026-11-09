//! A Sync Positions' window: the recording it syncs, how, and what it did.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use egui_plot::{Corner, Legend, Line, Plot, Points};

use super::files::{Plan, Recording};
use super::{Job, Report, Status, SyncPositions, Synced, file_name};
use crate::Edges;
use crate::standard::other::run_command::RunOnFiles;
use crate::standard::widgets::{setting, settings_grid};
use ephys_rs::engine::UiContext;
use ephys_rs::theme::Colours;

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
    pub(super) job: Job,
    preview: Option<Preview>,
}

impl SyncState {
    /// Showing what `job` is doing, or did.
    #[cfg(test)]
    pub(super) fn with(job: Job) -> Self {
        SyncState { job, preview: None }
    }

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

impl SyncPositions {
    pub(super) fn window(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        ui.horizontal(|ui| context.node_toolbar(ui));
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.body(ui, context));
    }

    fn body(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
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
                let seconds =
                    |ui: &mut egui::Ui, value: &mut f64, range: std::ops::RangeInclusive<f64>, hover: &str| {
                        let speed = range.end() / 500.0;
                        ui.add(egui::DragValue::new(value).range(range).speed(speed).prefix("± ").suffix(" s"))
                            .on_hover_text(hover)
                    };
                setting(ui, "Pair within", |ui| {
                    seconds(
                        ui,
                        &mut settings.within,
                        0.01..=10.0,
                        "How far apart, by their clocks, the same change can \
                        be on the board and by the camera",
                    )
                });
                let within = settings.within;
                setting(ui, "Alone within", |ui| {
                    seconds(
                        ui,
                        &mut settings.alone,
                        within..=30.0,
                        "Another change on the other side this near makes a \
                        change ambiguous, so it's not paired",
                    )
                });
                setting(ui, "Offset every", |ui| {
                    ui.add(egui::DragValue::new(&mut settings.step).range(1.0..=600.0).suffix(" s"))
                        .on_hover_text("How often through the recording the offset between the clocks is found")
                });
                setting(ui, "Median over", |ui| {
                    seconds(ui, &mut settings.around, 1.0..=1800.0, "Each offset is the median of the pairs' this near")
                });
                settings.alone = settings.alone.max(settings.within);
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
    let synced = &report.synced;
    for warning in &synced.warnings {
        ui.add(egui::Label::new(RichText::new(format!("⚠ {warning}")).color(theme.warning)).wrap());
    }
    ui.add_space(6.0);
    ui.columns(2, |columns| {
        stats(&mut columns[0], synced);
        pairing(&mut columns[1], synced);
    });
    ui.add_space(6.0);
    ui.strong("The board's clock ahead of the camera's");
    chart(ui, synced, theme);
}

fn stats(ui: &mut egui::Ui, synced: &Synced) {
    let edits = &synced.edits;
    egui::Grid::new("sync stats").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
        let row = |ui: &mut egui::Ui, what: &str, value: String| {
            ui.weak(what);
            ui.label(value);
            ui.end_row();
        };
        let positions = format!("{} recorded, {} written at {} Hz", synced.recorded, edits.positions, synced.rate);
        row(ui, "Positions", positions);
        let mut dropped = format!("{} sharing a slot", edits.dropped);
        if edits.before > 0 {
            dropped += &format!(", {} before the start", edits.before);
        }
        row(ui, "Dropped", dropped);
        let added = match edits.gaps {
            0 => "none".to_string(),
            1 => format!("{} in a gap", edits.added),
            gaps => format!("{} in {gaps} gaps, the longest {}", edits.added, edits.longest_gap),
        };
        row(ui, "Added, not found", added);
        let offsets: Vec<f64> = synced.knots.iter().filter_map(|k| k.offset).collect();
        if let (Some(first), Some(last)) = (offsets.first(), offsets.last()) {
            row(ui, "Offset", format!("{:.1} ms at the start, {:.1} ms at the end", first * 1e3, last * 1e3));
        }
        if let Some(ppm) = synced.drift_ppm {
            row(ui, "Drift", format!("{ppm:+.1} ppm"));
        }
        row(ui, "Scatter", format!("{:.2} ms, the pairs' median from the offset", synced.scatter * 1e3));
    });
}

/// What became of each side's changes.
fn pairing(ui: &mut egui::Ui, synced: &Synced) {
    egui::Grid::new("sync pairing").num_columns(4).spacing([12.0, 2.0]).show(ui, |ui| {
        ui.label("");
        for heading in ["Paired", "Ambiguous", "Unmatched"] {
            ui.weak(heading);
        }
        ui.end_row();
        let tallies = [("Board's line", synced.pairing.board), ("Timing LED", synced.pairing.led)];
        for (side, tally) in tallies {
            ui.weak(side);
            for count in [tally.paired, tally.ambiguous, tally.unmatched] {
                ui.label(count.to_string());
            }
            ui.end_row();
        }
    });
}

/// The offset through the recording, over the pairs it's the median of, in milliseconds.
fn chart(ui: &mut egui::Ui, synced: &Synced, theme: &Colours) {
    let pairs: Vec<[f64; 2]> = synced.pairing.pairs.iter().map(|p| [p.board, p.offset() * 1e3]).collect();
    // A line through the knots with an offset, broken where they've none.
    let mut runs = vec![Vec::new()];
    for knot in &synced.knots {
        match knot.offset {
            Some(offset) => runs.last_mut().unwrap().push([knot.board, offset * 1e3]),
            None if !runs.last().unwrap().is_empty() => runs.push(Vec::new()),
            None => {}
        }
    }
    Plot::new(ui.scope_id().with("offsets"))
        .height(200.0)
        .legend(Legend::default().position(Corner::LeftTop))
        .x_axis_formatter(|mark, _| format!("{} s", mark.value))
        .y_axis_formatter(|mark, _| format!("{} ms", mark.value))
        .show(ui, |plot| {
            plot.points(Points::new("Pairs", pairs).radius(1.5).color(theme.text_secondary));
            for run in runs.into_iter().filter(|r| !r.is_empty()) {
                plot.line(Line::new("Offset", run).color(theme.accent).width(2.0));
            }
        });
}

/// `names`, as a list of alternatives.
fn any_of(names: &[String]) -> String {
    match names {
        [others @ .., last] if !others.is_empty() => format!("{} or {last}", others.join(", ")),
        _ => names.concat(),
    }
}
