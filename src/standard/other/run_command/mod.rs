//! Runs a program on files, in a terminal of its own, whatever the graph's doing: by hand, on files picked here, or by
//! itself, on files sent to it with `RunOnFiles` (by a writer, once it's finished recording them, say). It's nothing
//! to do with passes: the files come as a direct trigger (see `crate::triggers`).
//!
//! The terminal's a process apart, running a script written here, so its output's there, not here, and it carries on if
//! the app quits. To be stopped it has to be findable, and what's started here often isn't what's running (macOS's `open`
//! hands the script to Terminal and goes): see `terminal`.
//!
//! The arguments are quoted in the script, so a path with spaces in it arrives whole, but pipes and `&&` mean nothing,
//! unless the program's a shell (`bash`, with `-c "..."`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::engine::{Category, Control, EdgeInfo, EdgeType, Node, NodeInfo, Output, SettingPort, UiContext};
use crate::gui::widgets::{Icon, help_link, small_icon_button};
use crate::standard::recording::expand;
use crate::standard::widgets::{combo, setting, settings_grid};
use crate::theme::Colours;
use crate::triggers::{ControlTriggers, Delivery, TriggerHandle, TriggerType};
use crate::{DataStream, Edges, Setting, SettingKind, Trigger, TriggerKind};

mod terminal;

use terminal::{Chain, Job, Outcome, Progress};

/// The trigger of a command setting, delivered directly (see `crate::triggers`): run it on these files. Whatever sends them,
/// and whenever: a writer's are what it's just finished recording.
#[derive(Clone, Debug, PartialEq)]
pub struct RunOnFiles(pub Vec<PathBuf>);

impl TriggerType for RunOnFiles {
    const NAME: &'static str = "run on files";
    const DELIVERY: Delivery = Delivery::Direct;
}

/// In the arguments: replaced by a file, running the program once for each.
const EACH_FILE: &str = "{file}";
/// In the arguments: replaced by all the files, running the program once.
const ALL_FILES: &str = "{files}";

/// How many times a program's run, and how.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Mode {
    /// Once for each file, one after the other.
    #[default]
    EachInTurn,
    /// Once for each file, all at the same time.
    EachTogether,
    /// Once, with all the files, if there are any, or none.
    Once,
}

/// When what's connected after it (to its "after running" control) is sent the files it ran on, once it's done. Never if
/// it was stopped.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Then {
    /// If every command succeeded.
    #[default]
    OnSuccess,
    /// If any failed.
    OnFailure,
    /// Either way.
    Always,
}

/// Its control, sending the files it ran on to what's connected, once it's done (see `Then`).
const AFTER_RUNNING: usize = 0;

/// What to run, and on what.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandSettings {
    /// Files, and folders standing for the files in them, to run it on by hand.
    pub files: Vec<PathBuf>,
    /// A regular expression: only files with a match for it in their names, of those here or from a writer. Empty's all
    /// of them.
    pub pattern: String,
    /// Whether `pattern`'s a regular expression, rather than text the name has in it.
    pub regex: bool,
    /// A name to look for on the PATH, or the exact path of a program.
    pub program: String,
    /// How many times it's run, and how.
    pub mode: Mode,
    /// Separated by spaces, with quotes around any that have spaces in them.
    pub arguments: String,
    /// Whether the terminal's left to close once everything's run, if nothing failed, rather than waiting to be read.
    pub close_on_success: bool,
    /// When what's connected after it runs.
    pub then: Then,
}

impl Default for CommandSettings {
    fn default() -> Self {
        let arguments = EACH_FILE.into();
        let (files, pattern, program) = (Vec::new(), String::new(), String::new());
        CommandSettings {
            files,
            pattern,
            regex: false,
            program,
            mode: Mode::default(),
            arguments,
            close_on_success: true,
            then: Then::default(),
        }
    }
}

impl EdgeType for CommandSettings {
    const INFO: EdgeInfo = EdgeInfo {
        name: "command",
        colour: Color32::from_rgb(190, 150, 220),
        description: "A program to run on files: what's connected to one can send it files to run on.",
    };

    fn summary(&self) -> String {
        if self.program.is_empty() { "no program".into() } else { self.program.clone() }
    }
}

impl CommandSettings {
    /// `paths` that match the pattern.
    fn matching(&self, paths: impl IntoIterator<Item = PathBuf>) -> Result<Vec<PathBuf>, String> {
        let pattern = self.pattern.trim();
        let pattern = if self.regex { pattern.to_string() } else { regex::escape(pattern) };
        let pattern = Regex::new(&pattern).map_err(|e| e.to_string())?;
        let matches = |path: &PathBuf| path.file_name().is_some_and(|name| pattern.is_match(&name.to_string_lossy()));
        Ok(paths.into_iter().filter(matches).collect())
    }

    /// The files picked here: those in folders, in order, then those on their own.
    fn listed(&self) -> Result<Vec<PathBuf>, String> {
        let mut paths = Vec::new();
        for path in &self.files {
            let path = expand(&path.to_string_lossy());
            if path.is_dir() {
                let entries = std::fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
                files.sort();
                paths.append(&mut files);
            } else {
                paths.push(path);
            }
        }
        Ok(paths)
    }

    /// Running the program on `files`: once for each, or once, by the mode.
    fn runs(&self, files: &[PathBuf]) -> Result<Vec<Run>, String> {
        let program = self.program.trim();
        if program.is_empty() {
            return Err("There's no program to run".into());
        }
        let program = expand(program);
        let arguments = split_arguments(&self.arguments)?;
        let run =
            |file: Option<&PathBuf>| Run { program: program.clone(), arguments: expanded(&arguments, file, files) };
        if self.mode == Mode::Once {
            if arguments.iter().any(|a| a.contains(EACH_FILE)) {
                return Err(format!("{EACH_FILE} is for one command per file: use {ALL_FILES}"));
            }
            return Ok(vec![run(None)]);
        }
        if files.is_empty() {
            return Err("No files match".into());
        }
        Ok(files.iter().map(|file| run(Some(file))).collect())
    }
}

/// `arguments`, with `file` for `EACH_FILE`, and `files` for `ALL_FILES`: as arguments of their own if it's one by itself.
fn expanded(arguments: &[String], file: Option<&PathBuf>, files: &[PathBuf]) -> Vec<String> {
    let text = |path: &PathBuf| path.to_string_lossy().into_owned();
    let all: Vec<String> = files.iter().map(text).collect();
    let mut out = Vec::new();
    for argument in arguments {
        if argument == ALL_FILES {
            out.extend(all.iter().cloned());
            continue;
        }
        let argument = argument.replace(ALL_FILES, &all.join(" "));
        out.push(file.map_or(argument.clone(), |file| argument.replace(EACH_FILE, &text(file))));
    }
    out
}

/// `text`'s arguments: separated by spaces, except within single or double quotes.
fn split_arguments(text: &str) -> Result<Vec<String>, String> {
    let (mut arguments, mut current, mut quote) = (Vec::new(), None::<String>, None);
    for c in text.chars() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => current.get_or_insert_default().push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                current.get_or_insert_default();
            }
            (None, c) if c.is_whitespace() => arguments.extend(current.take()),
            (None, c) => current.get_or_insert_default().push(c),
        }
    }
    if quote.is_some() {
        return Err("A quote in the arguments isn't closed".into());
    }
    arguments.extend(current);
    Ok(arguments)
}

/// Running a program once.
#[derive(Clone, PartialEq, Debug)]
struct Run {
    program: PathBuf,
    arguments: Vec<String>,
}

impl Run {
    /// As it might be typed.
    fn shown(&self) -> String {
        let quoted = |text: &str| if text.contains(' ') { format!("\"{text}\"") } else { text.to_string() };
        let mut words = vec![quoted(&self.program.to_string_lossy())];
        words.extend(self.arguments.iter().map(|a| quoted(a)));
        words.join(" ")
    }
}

/// What the settings come to: the files picked, and the commands, as they'd be shown.
struct Preview {
    settings: CommandSettings,
    connected: bool,
    made: Instant,
    /// How many files were picked, before any were left out by name, if they were, rather than sent.
    listed: Option<usize>,
    picked: Result<Vec<PathBuf>, String>,
    runs: Result<Vec<Run>, String>,
    shown: Vec<String>,
}

/// The last `Preview`, kept while the settings are the same, for a second at most: picking files reads folders, which
/// can change.
#[derive(Default)]
struct PreviewCache(Option<Arc<Preview>>);

impl PreviewCache {
    /// What `settings` come to, with an example file in place of those sent, if something that sends them is
    /// `connected`.
    fn of(&mut self, settings: &CommandSettings, connected: bool) -> Arc<Preview> {
        if let Some(preview) = self.0.as_ref()
            && preview.settings == *settings
            && preview.connected == connected
            && preview.made.elapsed() < Duration::from_secs(1)
        {
            return preview.clone();
        }
        let (listed, picked) = if connected {
            (None, settings.matching(Vec::new()).map(|_| vec![PathBuf::from("<file sent>")]))
        } else {
            let listed = settings.listed();
            (listed.as_ref().ok().map(Vec::len), listed.and_then(|paths| settings.matching(paths)))
        };
        let runs = picked.clone().and_then(|files| settings.runs(&files));
        let shown = runs.as_ref().map(|runs| runs.iter().map(Run::shown).collect()).unwrap_or_default();
        let (settings, made) = (settings.clone(), Instant::now());
        let preview = Arc::new(Preview { settings, connected, made, listed, picked, runs, shown });
        self.0 = Some(preview.clone());
        preview
    }
}

/// A Run Command's state on the GUI's side (its `UiState`): what it's running, or last ran, and what its settings come
/// to. Not settings, so each node has its own: a copy of one (duplicated, say) isn't running what it is.
#[derive(Default)]
pub struct CommandState {
    job: Job,
    preview: PreviewCache,
}

/// Runs a program on files.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunCommand {
    pub settings: CommandSettings,
}

impl RunCommand {
    /// Runs the program on `files` as `job`, or says why it can't; then sends them on through `after`, if that's
    /// connected, as the settings say (see `Then`).
    fn run_on(&self, job: &Job, files: Result<Vec<PathBuf>, String>, after: Option<TriggerHandle<Trigger>>) {
        let runs = files.and_then(|files| Ok((self.settings.runs(&files)?, files)));
        match runs {
            Ok((runs, files)) => {
                let chain = after.map(|handle| Chain { handle, then: self.settings.then, files });
                job.start(&self.settings, &runs, chain)
            }
            Err(problem) => job.fail(problem),
        }
    }

    fn window(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        ui.horizontal(|ui| context.node_toolbar(ui));
        ui.horizontal(|ui| {
            ui.label("Runs a program on files, in a terminal window of its own.");
            help_link(ui, "Examples", examples);
        });
        let warning = "Recording performance may suffer if the computer's resources are strained by other running \
                       commands.";
        ui.label(RichText::new(warning).color(context.theme.warning));
        ui.add_space(6.0);
        let job = context.ui_state::<CommandState>().job.clone();
        let (locked, running) = (context.locked, job.running());
        // What's connected supplies all the files, while it is.
        let senders = context.controlled_by.first().map_or(&[][..], Vec::as_slice);
        let connected = !senders.is_empty();
        let preview = context.ui_state::<CommandState>().preview.of(&self.settings, connected);
        // Nothing changes while it's running, to what it shows as running.
        // How many files picked here match, of how many.
        let matched = preview.picked.as_ref().ok().map(Vec::len);
        let counts = preview.listed.zip(matched).map(|(listed, matched)| (matched, listed));
        let chained = context.control_connected(AFTER_RUNNING);
        self.settings_section(ui, senders, counts, chained, locked || running);
        ui.add_space(6.0);
        let (picked, runs) = (&preview.picked, &preview.runs);
        let results = job.runs_with(&self.settings);
        ui.separator();
        ui.horizontal(|ui| {
            // How many there are: not for an example, as that's up to what's sent.
            let count = match (&results, runs) {
                (Some(results), _) => format!(" ({})", results.len()),
                (None, Ok(runs)) if !connected => format!(" ({})", runs.len()),
                _ => String::new(),
            };
            ui.strong(match &results {
                Some(_) if running => format!("Commands{count} · running"),
                Some(_) => format!("Commands{count} · last run"),
                None if connected => "Commands · an example".into(),
                None => format!("Commands{count}"),
            });
            ui.add_space(6.0);
            // Whatever's locked, something heavy can always be stopped.
            if running {
                if ui.button("Stop").on_hover_text("Stop what's running in its terminal").clicked() {
                    job.stop();
                }
            } else {
                let run = ui.add_enabled(runs.is_ok() && !locked && !connected, egui::Button::new("Run"));
                let run = if connected {
                    run.on_disabled_hover_text("It runs by itself on the files sent")
                } else {
                    run.on_hover_text("Open a terminal, and run these there")
                };
                // As if it were sent them: so it's the same, after, either way.
                if run.clicked()
                    && let Ok(files) = picked
                {
                    context.trigger(None, RunOnFiles(files.clone()));
                }
                if connected {
                    ui.weak(format!("Run triggered by {}", any_of(senders)));
                }
            }
        });
        ui.add_space(2.0);
        let code = egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8, 6));
        code.show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Only the rows showing are laid out, so a row's a line: there may be hundreds.
            let list = egui::ScrollArea::vertical().max_height(180.0).auto_shrink([false, true]);
            let row_height =
                ui.text_style_height(&egui::TextStyle::Monospace).max(ui.text_style_height(&egui::TextStyle::Body));
            match (&results, runs) {
                (Some(results), _) => {
                    list.show_rows(ui, row_height, results.len(), |ui, rows| {
                        results[rows].iter().for_each(|progress| progress_row(ui, progress, context.theme));
                    });
                }
                (None, Ok(_)) => {
                    list.show_rows(ui, row_height, preview.shown.len(), |ui, rows| {
                        for shown in &preview.shown[rows] {
                            ui.add(egui::Label::new(RichText::new(shown).monospace()).truncate()).on_hover_text(shown);
                        }
                    });
                }
                (None, Err(problem)) => {
                    ui.weak(problem);
                }
            }
        });
        if let Some(problem) = job.problem() {
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(problem).color(context.theme.warning)).wrap());
        }
        if running {
            // It ends elsewhere, with nothing to tell the GUI.
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    /// The files to run it on by hand, unless `senders` supply them, then the rest of the settings.
    /// `counts` are how many files picked here match the name, of how many, if they're known.
    fn settings_section(
        &mut self,
        ui: &mut egui::Ui,
        senders: &[String],
        counts: Option<(usize, usize)>,
        chained: bool,
        disabled: bool,
    ) {
        let listed = counts.map(|(_, listed)| listed);
        let settings = &mut self.settings;
        ui.add_enabled_ui(!disabled, |ui| {
            settings_grid(ui, |ui| {
                let label = ui.label("Files");
                if senders.is_empty() {
                    ui.vertical(|ui| files_list(ui, &mut settings.files, listed)).response.labelled_by(label.id);
                    ui.end_row();
                    help(ui, &[&files_help(senders)]);
                } else {
                    // Those picked are kept, for when it's not connected.
                    ui.weak(files_help(senders));
                    ui.end_row();
                }
                let label = ui.label("Only files named");
                ui.horizontal(|ui| {
                    let hint = if settings.regex {
                        "Only files with names that match this regular expression"
                    } else {
                        "Only files with this in their names"
                    };
                    // Narrower, to leave room for what's beside it.
                    let width = |ui: &egui::Ui, text: &str| {
                        let font = egui::TextStyle::Body.resolve(ui.style());
                        ui.painter().layout_no_wrap(text.into(), font, Color32::WHITE).size().x
                    };
                    let gap = ui.spacing().item_spacing.x;
                    let mut beside = ui.spacing().interact_size.y + 2.0 * gap + 12.0;
                    if settings.regex {
                        beside += width(ui, "Cheat sheet") + gap;
                    }
                    if let Some((matched, listed)) = counts {
                        beside += width(ui, &format!("matches {matched}/{listed}")) + 2.0 * gap;
                    }
                    let edit = egui::TextEdit::singleline(&mut settings.pattern)
                        .desired_width((ui.available_width() - beside).clamp(80.0, ui.spacing().text_edit_width));
                    ui.add(edit).labelled_by(label.id).on_hover_text(hint);
                    // Framed whether it's on or not, so it looks like something to press.
                    let toggle = egui::Button::new(".*").selected(settings.regex);
                    if ui.add(toggle).on_hover_text("Regular expression").clicked() {
                        settings.regex = !settings.regex;
                    }
                    if settings.regex {
                        help_link(ui, "Cheat sheet", regex_cheat_sheet);
                    }
                    if let Some((matched, listed)) = counts {
                        ui.add_space(4.0);
                        ui.weak(format!("matches {matched}/{listed}"));
                    }
                });
                ui.end_row();
                let label = ui.label("Program");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut settings.program).labelled_by(label.id);
                    if ui.button("Browse…").clicked()
                        && let Some(program) = rfd::FileDialog::new().pick_file()
                    {
                        settings.program = program.display().to_string();
                    }
                });
                ui.end_row();
                help(ui, &["A name on your PATH, like python3, or Browse to the file"]);
                let modes = [
                    (Mode::EachInTurn, "One per file, in turn"),
                    (Mode::EachTogether, "One per file, in parallel"),
                    (Mode::Once, "Just one"),
                ];
                setting(ui, "Mode", |ui| combo(ui, "mode", &mut settings.mode, &modes));
                setting(ui, "Arguments", |ui| ui.text_edit_singleline(&mut settings.arguments));
                help(
                    ui,
                    &[match settings.mode {
                        Mode::Once => "{files} is replaced by all the files, if any",
                        Mode::EachInTurn | Mode::EachTogether => "{file} is replaced by the file",
                    }],
                );
                setting(ui, "Terminal", |ui| {
                    ui.checkbox(&mut settings.close_on_success, "Close if every command succeeds").on_hover_text(
                        "Otherwise it waits, so what they said can be read. It always does if one fails.",
                    )
                });
                // Only with something connected after it to run.
                if chained {
                    let thens = [
                        (Then::OnSuccess, "If every command succeeds"),
                        (Then::OnFailure, "If any fails"),
                        (Then::Always, "Whether they succeed or fail"),
                    ];
                    setting(ui, "Then", |ui| combo(ui, "then", &mut settings.then, &thens));
                    help(ui, &["Runs what's connected after it on the same files. Not if it's stopped"]);
                }
            });
        });
    }
}

/// A run in a batch, with how it went, or is going: a mark, how long it took, and the exit code if it failed.
fn progress_row(ui: &mut egui::Ui, progress: &Progress, theme: &Colours) {
    let (mark, colour, time) = match progress.outcome {
        Outcome::Waiting => ("•", theme.text_disabled, String::new()),
        Outcome::Running(started) => ("", theme.text_primary, duration(started.elapsed())),
        Outcome::Ended { took, code: Some(0) } => ("✔", theme.success, duration(took)),
        Outcome::Ended { took, code: Some(code) } => ("✖", theme.error, format!("{} · exit {code}", duration(took))),
        Outcome::Ended { took, code: None } => ("✖", theme.error, format!("stopped at {}", duration(took))),
        Outcome::NotRun => ("–", theme.text_disabled, "not run".into()),
    };
    ui.horizontal(|ui| {
        let height = ui.text_style_height(&egui::TextStyle::Monospace);
        let size = egui::vec2(14.0, height);
        ui.allocate_ui_with_layout(size, egui::Layout::centered_and_justified(egui::Direction::LeftToRight), |ui| {
            if matches!(progress.outcome, Outcome::Running(_)) {
                ui.add(egui::Spinner::new().size(height * 0.8));
            } else {
                ui.label(RichText::new(mark).color(colour));
            }
        });
        let size = egui::vec2(110.0, height);
        ui.allocate_ui_with_layout(size, egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.weak(time);
        });
        let command = egui::Label::new(RichText::new(&progress.shown).monospace()).truncate();
        ui.add(command).on_hover_text(&progress.shown);
    });
}

/// How long something took, to a tenth of a second, as people say it.
fn duration(took: Duration) -> String {
    let seconds = took.as_secs_f64();
    if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else {
        format!("{} min {:02} s", took.as_secs() / 60, took.as_secs() % 60)
    }
}

/// The files picked, a few rows at a time, to take off the list, and buttons to pick others: files, from one folder, or
/// a folder, standing for the files in it.
fn files_list(ui: &mut egui::Ui, files: &mut Vec<PathBuf>, listed: Option<usize>) {
    const ROWS: usize = 3;
    let mut remove = None;
    if !files.is_empty() {
        // As tall as the button to take one off, at least.
        let row_height = ui.spacing().interact_size.y.max(20.0);
        let spacing = ui.spacing().item_spacing.y;
        // A row cut in half says there are more.
        let rows = if files.len() > ROWS { ROWS as f32 - 0.5 } else { files.len() as f32 };
        let height = rows * (row_height + spacing) - spacing;
        let list = egui::ScrollArea::vertical()
            .id_salt("files")
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink([false, true]);
        list.show_rows(ui, row_height, files.len(), |ui, rows| {
            for index in rows {
                ui.horizontal(|ui| {
                    let label = format!("Delete file {}", index + 1);
                    if small_icon_button(ui, Icon::Delete, &label, "Take this off the list").clicked() {
                        remove = Some(index);
                    }
                    let path = files[index].display().to_string();
                    ui.add(egui::Label::new(&path).truncate()).on_hover_text(path);
                });
            }
        });
    }
    if let Some(index) = remove {
        files.remove(index);
    }
    ui.horizontal(|ui| {
        if ui.button("Select files…").on_hover_text("Files from one folder, in place of those here").clicked()
            && let Some(paths) = rfd::FileDialog::new().pick_files()
        {
            *files = paths;
        }
        if ui.button("Select a folder…").on_hover_text("The files in it, in place of those here").clicked()
            && let Some(folder) = rfd::FileDialog::new().pick_folder()
        {
            *files = vec![folder];
        }
        let count = |n: usize| if n == 1 { "1 file".to_string() } else { format!("{n} files") };
        let count = match (&files[..], listed) {
            ([], _) => String::new(),
            ([folder], Some(listed)) if folder.is_dir() => format!("1 folder · {}", count(listed)),
            ([folder], None) if folder.is_dir() => "1 folder".into(),
            (files, _) => count(files.len()),
        };
        ui.weak(count);
    });
}

/// What it's for, with how it'd be set up for a script of your own.
fn examples(ui: &mut egui::Ui) {
    ui.set_max_width(440.0);
    ui.label(
        "For processing recordings once they're made: spike sorting, converting or analysing them. The program can be \
         one made for the job, or a script of your own.",
    );
    let example = |ui: &mut egui::Ui, title: &str, program: &str, arguments: &str, note: &str| {
        ui.add_space(6.0);
        ui.strong(title);
        ui.horizontal(|ui| {
            ui.label("Program");
            ui.label(RichText::new(program).monospace());
        });
        ui.horizontal(|ui| {
            ui.label("Arguments");
            ui.label(RichText::new(arguments).monospace());
        });
        ui.weak(note);
    };
    example(
        ui,
        "A Python script",
        "python3",
        "/path/to/my_script.py {file}",
        "In the script, the file is sys.argv[1].",
    );
    example(
        ui,
        "A MATLAB function",
        "matlab",
        "-sd /path/to/scripts -batch \"my_function('{file}')\"",
        "MATLAB starts in /path/to/scripts (-sd), the folder my_function.m is in, so it can find it. -batch needs \
         MATLAB R2019a or later. MATLAB often isn't on the PATH: Browse to it, in its bin folder.",
    );
}

/// A row of a settings grid with `lines` of help, under what they're about.
fn help(ui: &mut egui::Ui, lines: &[&str]) {
    ui.label("");
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for line in lines {
            ui.weak(*line);
        }
    });
    ui.end_row();
}

/// The bits of regular expressions most likely wanted.
fn regex_cheat_sheet(ui: &mut egui::Ui) {
    const BITS: [(&str, &str); 10] = [
        (r"\d", "digit"),
        ("[a-z]", "letter"),
        (r"\.", "literal dot"),
        (".", "any character"),
        ("?", "optional"),
        ("+", "one or more"),
        ("*", "zero or more"),
        ("a|b", "a or b"),
        ("^", "start"),
        ("$", "end"),
    ];
    // Which kind, so it can be looked up: the regex crate's, as `CommandSettings::matching` uses.
    ui.set_max_width(260.0);
    ui.label("Rust regex syntax: like Perl's or Python's, without look-around or backreferences.");
    ui.add_space(4.0);
    egui::Grid::new("regular expressions").spacing([10.0, 2.0]).show(ui, |ui| {
        for (bit, meaning) in BITS {
            ui.label(RichText::new(bit).monospace());
            ui.label(meaning);
            ui.end_row();
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

/// Where the files come from, given the names of what's connected to supply them.
fn files_help(senders: &[String]) -> String {
    if senders.is_empty() {
        "Connect a supporting node to supply files automatically".into()
    } else {
        format!("Supplied by {}: disconnect or disable it to pick files yourself", any_of(senders))
    }
}

impl Node<Edges> for RunCommand {
    type State = ();
    type Shared = ();
    type UiState = CommandState;
    type GlobalSettings = ();

    const INFO: NodeInfo = NodeInfo {
        name: "Run Command",
        category: Category::Other,
        description: "Runs a program on files, in a terminal: by hand, or on files sent to it (by a writer, after recording).",
    };

    const WINDOW_SIZE: [f32; 2] = [820.0, 520.0];
    const SHOW_IN_BASIC_DEFAULT: bool = true;

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "command", value: self.settings.clone().into(), on_right: false }]
    }

    /// What's connected after it, run on the same files once it's done (see `Then`).
    fn controls(&self) -> Vec<Control<Edges>> {
        vec![Control { name: "after running", kind: SettingKind::Command, read_only: true, on_right: true }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Command(settings) = value {
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

    /// Runs the program on the files sent (by hand, too), and then, as it says, what's connected after it.
    fn on_trigger(&mut self, trigger: Trigger, context: &mut UiContext<Edges>, triggers: &ControlTriggers<Trigger>) {
        if let Trigger::RunOnFiles(RunOnFiles(paths)) = trigger {
            let (files, after) =
                (self.settings.matching(paths), triggers.handle(AFTER_RUNNING, TriggerKind::RunOnFiles));
            self.run_on(&context.ui_state::<CommandState>().job, files, after);
        }
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        self.window(ui, context);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    use super::terminal::{How, shell_script, write_private};
    use super::*;
    use crate::NodeType;
    use crate::Nodes;
    use crate::engine::NodeTypeEnum;
    use crate::engine::{Graph, NodeId, NodesEnum, Port, PortRole, Recording, Runner, Snapshot};
    use crate::standard::ephys::board::{Source, serve_test_board};
    use crate::standard::recording::Location;
    use crate::standard::themes::THEMES;
    use crate::triggers::{DirectTriggers, Triggered};

    /// One per file, in turn, if the arguments have a file in them, otherwise just one.
    fn settings(program: &str, arguments: &str) -> CommandSettings {
        let mode = if arguments.contains(EACH_FILE) { Mode::EachInTurn } else { Mode::Once };
        let (program, arguments) = (program.into(), arguments.into());
        CommandSettings { program, arguments, mode, ..Default::default() }
    }

    fn outcomes(job: &Job, settings: &CommandSettings) -> Vec<Outcome> {
        job.runs_with(settings).unwrap().iter().map(|progress| progress.outcome).collect()
    }

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(done(), "{what}");
    }

    /// Sends `node` `trigger`, as the app does, with a context and a `UiState` of its own; what it ran.
    fn send(node: RunCommand, trigger: Trigger, triggers: &ControlTriggers<Trigger>) -> Job {
        let mut graph = Graph::<Nodes>::default();
        let id = graph.add(Nodes::RunCommand(node), [0.0, 0.0]);
        let state: crate::engine::UiStateCell = std::cell::RefCell::new(Box::new(CommandState::default()));
        let colours = &THEMES[0].colours;
        graph.edit(id, &[], colours, None, Some(&state), None, false, false, None, |node, context| {
            node.on_trigger(trigger, context, triggers)
        });
        state.borrow().downcast_ref::<CommandState>().unwrap().job.clone()
    }

    fn wait_for_job(job: &Job) {
        wait_until("it should have finished", || !job.running());
    }

    /// Run (by hand, or sent files), what's connected after it is sent the files once it's done.
    #[test]
    fn what_runs_after_is_sent_the_files_it_ran_on_once_its_done() {
        let direct = Arc::new(DirectTriggers::default());
        let kinds: &'static [TriggerKind] = &[TriggerKind::RunOnFiles];
        let triggers = ControlTriggers::<Trigger>::new(direct.clone(), vec![vec![(NodeId(9), kinds)]]);
        let node = RunCommand { settings: settings("/bin/sleep", "0.2") };
        let job = send(node, Trigger::RunOnFiles(RunOnFiles(paths(&["a.dat"]))), &triggers);
        assert!(direct.take().is_empty(), "not till it's done");
        wait_for_job(&job);
        let sent = direct.take();
        let [(NodeId(9), Triggered::Trigger(trigger))] = &sent[..] else { panic!("sent once, to what's after it") };
        assert_eq!(trigger.key(), TriggerKind::RunOnFiles.into());
    }

    /// A path to have a program leave something at.
    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("ephys-rs-run-test-{}-{number}-{name}", std::process::id()))
    }

    /// What `runs` say in a terminal, from `shell_script`, run as a terminal runs it, but with nobody to press return.
    fn said(runs: &[Run]) -> String {
        said_closing(runs, false)
    }

    /// What `runs` say, with the terminal left to close if they all succeed, if `close`.
    fn said_closing(runs: &[Run], close: bool) -> String {
        let (script, pid_file, status_file) = (scratch("script"), scratch("pid"), scratch("status"));
        std::fs::write(
            &script,
            shell_script(
                &std::env::temp_dir(),
                &pid_file,
                &status_file,
                runs,
                How { close, close_window: close, ..How::default() },
            ),
        )
        .unwrap();
        let output = Command::new("/bin/sh").arg(&script).stdin(Stdio::null()).output().unwrap();
        assert!(output.status.success());
        assert!(!script.exists(), "it deletes itself");
        assert!(!pid_file.exists(), "it says it's ended");
        let status = std::fs::read_to_string(&status_file).unwrap();
        let ends = (0..runs.len()).map(|index| format!("end {index} ")).collect::<Vec<_>>();
        assert!(ends.iter().all(|end| status.contains(end.as_str())), "{status}");
        std::fs::remove_file(status_file).unwrap();
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn arguments_split_on_spaces_except_in_quotes() {
        assert_eq!(split_arguments("  a  'b c' \"d e\"f \"\" ").unwrap(), ["a", "b c", "d ef", ""]);
        assert!(split_arguments("a 'b").is_err(), "not closed");
        assert!(split_arguments("").unwrap().is_empty());
    }

    #[test]
    fn it_runs_once_for_each_file_or_just_once() {
        let files = paths(&["a b.set", "c.set"]);
        let each = settings("klusta", "{file} --out x{file}").runs(&files).unwrap();
        assert_eq!(each.len(), 2);
        assert_eq!(each[0].arguments, ["a b.set", "--out", "xa b.set"], "a path with a space in it stays whole");
        assert_eq!(each[1].arguments, ["c.set", "--out", "xc.set"]);
        let together = CommandSettings { mode: Mode::EachTogether, ..settings("klusta", "{file}") };
        assert_eq!(together.runs(&files).unwrap().len(), 2);
        let once = settings("klusta", "-v {files}").runs(&files).unwrap();
        assert_eq!(once.len(), 1);
        assert_eq!(once[0].arguments, ["-v", "a b.set", "c.set"]);
        assert_eq!(settings("klusta", "--all").runs(&files).unwrap().len(), 1, "it needn't be given them");
        let alone = settings("klusta", "-v {files}").runs(&[]).unwrap();
        assert_eq!(alone[0].arguments, ["-v"], "or have any");
        assert!(settings("klusta", "{file}").runs(&[]).is_err(), "nothing to run each on");
        assert!(CommandSettings { mode: Mode::Once, ..settings("klusta", "{file}") }.runs(&files).is_err(), "which?");
        assert!(settings(" ", "{file}").runs(&files).is_err(), "no program");
        let matlab = settings("matlab", "-sd /path/to/scripts -batch \"my_function('{file}')\"").runs(&files).unwrap();
        let expected = ["-sd", "/path/to/scripts", "-batch", "my_function('a b.set')"];
        assert_eq!(matlab[0].arguments, expected, "as in the examples");
    }

    #[test]
    fn only_files_whose_names_match_are_used() {
        let mut command = settings("x", "{file}");
        command.pattern = r"\.set$".into();
        command.regex = true;
        let files = paths(&["/some.set/trial.egf", "/here/trial.set", "/here/other.1"]);
        assert_eq!(command.matching(files.clone()).unwrap(), paths(&["/here/trial.set"]), "the name, not the folder");
        command.pattern = r"\.\d+$".into();
        let tetrodes = paths(&["/here/trial.1", "/here/trial.12", "/here/trial.set", "/here/trial.1.bak"]);
        assert_eq!(
            command.matching(tetrodes).unwrap(),
            paths(&["/here/trial.1", "/here/trial.12"]),
            "the LTX writer's"
        );
        command.pattern = String::new();
        assert_eq!(command.matching(files.clone()).unwrap(), files, "no pattern's all of them");
        command.regex = false;
        command.pattern = ".1".into();
        let names = paths(&["/here/trial.1", "/here/trial.12", "/here/trial11"]);
        assert_eq!(command.matching(names).unwrap(), paths(&["/here/trial.1", "/here/trial.12"]), "only the text");
        command.regex = true;
        command.pattern = "[".into();
        assert!(command.matching(files).is_err());
    }

    #[test]
    fn a_folder_stands_for_the_files_in_it() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-run-command-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        for name in ["b.set", "a.set", "c.egf"] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let mut command = settings("x", "{file}");
        command.pattern = r"\.set$".into();
        command.regex = true;
        command.files = vec![dir.clone()];
        assert_eq!(command.matching(command.listed().unwrap()).unwrap(), vec![dir.join("a.set"), dir.join("b.set")]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_terminal_shows_what_a_program_says_and_how_it_ended() {
        let said = said(&settings("/bin/sh", "-c 'echo out; exit 3'").runs(&[]).unwrap());
        let expected =
            "$ /bin/sh -c \"echo out; exit 3\"\nout\nFailed, exit status 3: /bin/sh -c \"echo out; exit 3\"\n";
        assert!(said.contains(expected), "{said}");
        assert!(said.contains("press return"), "{said}");
    }

    #[test]
    fn a_terminal_is_left_to_close_only_if_every_command_succeeds() {
        let ok = settings("/bin/sh", "-c 'exit 0' {file}").runs(&paths(&["a", "b"])).unwrap();
        assert!(!said_closing(&ok, true).contains("press return"));
        assert!(said_closing(&ok, false).contains("press return"), "unless it's not to");
        // Its window's closed only from a terminal: there's none here, so it's left alone.
        let script = shell_script(
            Path::new("/"),
            Path::new("/p"),
            Path::new("/s"),
            &ok,
            How { close: true, close_window: true, ..How::default() },
        );
        assert!(script.contains("osascript") && script.contains("case $tty in /dev/*)"), "{script}");
        for code in ["3", "10", "255"] {
            let failing = settings("/bin/sh", "-c 'exit \"$0\"' {file}").runs(&paths(&["0", code])).unwrap();
            assert!(said_closing(&failing, true).contains("press return"), "{code}");
        }
    }

    #[test]
    fn a_terminal_gets_arguments_as_they_are() {
        let tricky = paths(&["it's a $HOME `file`.set", "two  spaces \"quoted\"; rm"]);
        let said = said(&settings("/usr/bin/printf", "[%s] {files}").runs(&tricky).unwrap());
        assert!(said.contains("[it's a $HOME `file`.set][two  spaces \"quoted\"; rm]"), "{said}");
    }

    #[test]
    fn a_terminal_runs_them_in_turn_whether_they_fail_or_not() {
        let said =
            said(&settings("/bin/sh", "-c 'echo \"$0\"; exit 1' {file}").runs(&paths(&["first", "second"])).unwrap());
        let at = |text: &str| said.find(text).unwrap();
        assert!(at("\nfirst\n") < at("\nsecond\n"), "{said}");
    }

    #[test]
    fn a_terminal_runs_them_where_the_app_is() {
        let said = said(&settings("/bin/pwd", "").runs(&[]).unwrap());
        let folder = std::env::temp_dir().canonicalize().unwrap();
        assert!(said.lines().nth(1).unwrap().ends_with(folder.file_name().unwrap().to_str().unwrap()), "{said}");
    }

    #[test]
    fn scripts_are_only_for_this_user_and_can_be_run() {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch("private");
        write_private(&path, "text", 0o700).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(write_private(&path, "other", 0o700).is_err(), "not one that's there");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "text");
        std::fs::remove_file(path).unwrap();
    }

    /// A job with a program that won't end by itself going, then another waiting, and where they leave word.
    fn going() -> (Job, PathBuf, PathBuf) {
        let (first, second) = (scratch("first"), scratch("second"));
        let script = "-c 'echo $$ > \"$1\"; touch \"$1\".started; sleep 30' sh {file}";
        let mut runs = settings("/bin/sh", script).runs(std::slice::from_ref(&first)).unwrap();
        runs.extend(settings("/usr/bin/touch", "{file}").runs(std::slice::from_ref(&second)).unwrap());
        let job = Job::default();
        job.start(&CommandSettings::default(), &runs, None);
        let started = first.with_file_name(format!("{}.started", first.file_name().unwrap().to_str().unwrap()));
        wait_until("it should have started", || started.exists());
        std::fs::remove_file(started).unwrap();
        (job, first, second)
    }

    fn alive(pid: &str) -> bool {
        Command::new("kill").args(["-0", pid]).stderr(Stdio::null()).status().unwrap().success()
    }

    #[test]
    fn it_knows_while_a_terminal_is_going() {
        let (job, first, _) = going();
        assert!(job.running());
        job.stop();
        wait_for_job(&job);
        std::fs::remove_file(first).unwrap();
    }

    #[test]
    fn stopping_stops_what_is_running_and_what_is_waiting() {
        let (job, first, second) = going();
        let pid = std::fs::read_to_string(&first).unwrap();
        let pid = pid.trim();
        assert!(alive(pid));
        job.stop();
        wait_for_job(&job);
        let outcomes = outcomes(&job, &CommandSettings::default());
        assert!(matches!(outcomes[..], [Outcome::Ended { code: None, .. }, Outcome::NotRun]), "{outcomes:?}");
        wait_until("the program itself should have stopped, not just the script", || !alive(pid));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!second.exists());
        std::fs::remove_file(first).unwrap();
    }

    #[test]
    fn what_comes_after_stopping_still_runs() {
        let (job, first, _) = going();
        job.stop();
        wait_for_job(&job);
        let after = scratch("after");
        job.start(
            &CommandSettings::default(),
            &settings("/usr/bin/touch", "{file}").runs(std::slice::from_ref(&after)).unwrap(),
            None,
        );
        wait_until("it should have run", || after.exists());
        wait_for_job(&job);
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(after).unwrap();
    }

    #[test]
    fn nothing_else_runs_while_it_is_running() {
        let (job, first, _) = going();
        let other = scratch("other");
        job.start(
            &CommandSettings::default(),
            &settings("/usr/bin/touch", "{file}").runs(std::slice::from_ref(&other)).unwrap(),
            None,
        );
        assert!(job.problem().unwrap().contains("still running"));
        assert_eq!(job.runs_with(&CommandSettings::default()).unwrap().len(), 2, "what's running is shown");
        std::thread::sleep(Duration::from_millis(200));
        assert!(!other.exists());
        job.stop();
        wait_for_job(&job);
        std::fs::remove_file(first).unwrap();
    }

    #[test]
    fn it_times_each_run_and_keeps_its_exit_code() {
        let command = settings("/bin/sh", "-c 'sleep 0.3; exit \"$0\"' {file}");
        let job = Job::default();
        job.start(&command, &command.runs(&paths(&["0", "3"])).unwrap(), None);
        wait_for_job(&job);
        let outcomes = outcomes(&job, &command);
        let [Outcome::Ended { took, code: Some(0) }, Outcome::Ended { code: Some(3), .. }] = outcomes[..] else {
            panic!("{outcomes:?}")
        };
        assert!(took >= Duration::from_millis(250) && took < Duration::from_secs(5), "{took:?}");
        let mut changed = command.clone();
        changed.program = "/bin/bash".into();
        assert!(job.runs_with(&changed).is_none(), "shown till the settings change");
    }

    #[test]
    fn in_parallel_they_run_at_the_same_time() {
        let (waits_for, arrives) = (scratch("wait"), scratch("arrive"));
        // The first only succeeds if the second has run while it's still waiting.
        let wait = format!(
            "-c 'for i in $(seq 100); do [ -e \"$0\" ] && exit 0; sleep 0.05; done; exit 1' {}",
            waits_for.display()
        );
        let mut runs = settings("/bin/sh", &wait).runs(&[]).unwrap();
        runs.extend(settings("/usr/bin/touch", "{files}").runs(std::slice::from_ref(&waits_for)).unwrap());
        runs.extend(settings("/usr/bin/touch", "{files}").runs(std::slice::from_ref(&arrives)).unwrap());
        let command = CommandSettings { mode: Mode::EachTogether, ..CommandSettings::default() };
        let job = Job::default();
        job.start(&command, &runs, None);
        wait_for_job(&job);
        let outcomes = outcomes(&job, &command);
        assert!(outcomes.iter().all(|o| matches!(o, Outcome::Ended { code: Some(0), .. })), "{outcomes:?}");
        std::fs::remove_file(waits_for).unwrap();
        std::fs::remove_file(arrives).unwrap();
    }

    #[test]
    fn it_says_where_the_files_come_from_by_what_is_connected() {
        let names = |names: &[&str]| names.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        assert!(files_help(&[]).starts_with("Connect a supporting node"));
        assert!(files_help(&names(&["Write LTX"])).starts_with("Supplied by Write LTX:"));
        assert!(files_help(&names(&["A", "B", "C"])).starts_with("Supplied by A, B or C:"));
    }

    #[test]
    fn a_folder_says_how_many_files_are_in_it() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-run-command-count-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.set", "b.set", "c.egf"] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let mut files = vec![dir.clone()];
        let mut harness = egui_kittest::Harness::builder().with_size([400.0, 200.0]).build_ui(|ui| {
            files_list(ui, &mut files, Some(3));
        });
        harness.run_steps(2);
        use egui_kittest::kittest::Queryable;
        assert!(harness.query_by_label("1 folder · 3 files").is_some());
        drop(harness);
        let mut command = settings("x", "{file}");
        (command.files, command.pattern) = (vec![dir.clone()], ".set".into());
        let preview = PreviewCache::default().of(&command, false);
        assert_eq!((preview.listed, preview.shown.len()), (Some(3), 2), "all of them, before the name's looked at");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn what_the_settings_come_to_is_kept_till_they_change() {
        let mut cache = PreviewCache::default();
        let mut command = settings("klusta", "{file}");
        command.files = paths(&["/a.set", "/b.set"]);
        let first = cache.of(&command, false);
        assert_eq!(first.shown.len(), 2);
        assert!(Arc::ptr_eq(&first, &cache.of(&command, false)), "the same, so kept");
        assert!(!Arc::ptr_eq(&first, &cache.of(&command, true)), "connected, it's an example");
        command.files.pop();
        let fewer = cache.of(&command, false);
        assert_eq!(fewer.shown.len(), 1, "changed, so made again");
    }

    #[test]
    fn a_problem_is_shown_until_it_runs_again() {
        let (mut node, job) = (RunCommand::default(), Job::default());
        node.run_on(&job, Ok(Vec::new()), None);
        assert_eq!(job.problem().as_deref(), Some("There's no program to run"));
        node.settings = settings("/usr/bin/true", "");
        node.run_on(&job, Ok(Vec::new()), None);
        assert_eq!(job.problem(), None);
        wait_for_job(&job);
    }

    /// Real passes and a real recording: a writer's files go to what's connected to it, once they're finished, and only
    /// those that match.
    #[test]
    fn a_writer_has_it_run_on_what_it_recorded() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-after-recording-{}", std::process::id()));
        let said = std::env::temp_dir().join(format!("ephys-rs-after-recording-{}.txt", std::process::id()));
        let add = |graph: &mut Graph<Nodes>, name: &str| {
            let node_type = NodeType::named(name).unwrap();
            graph.add(Nodes::create(node_type), [0.0, 0.0])
        };
        let mut graph = Graph::default();
        let (board, writer, command) =
            (add(&mut graph, "Acquisition Board"), add(&mut graph, "Write Raw"), add(&mut graph, "Run Command"));
        if let Some(Nodes::AcquisitionBoard(b)) = graph.get_mut(board).map(|e| &mut e.node) {
            b.source = Source::Fake { address: serve_test_board() };
        }
        if let Some(Nodes::WriteRaw(w)) = graph.get_mut(writer).map(|e| &mut e.node) {
            w.location = Location::Directory { path: dir.display().to_string() };
        }
        if let Some(Nodes::RunCommand(c)) = graph.get_mut(command).map(|e| &mut e.node) {
            let arguments = format!("-c 'echo \"$0\" >> \"$1\"; wc -c < \"$0\" >> \"$1\"' {{file}} {}", said.display());
            c.settings = settings("/bin/sh", &arguments);
            c.settings.pattern = r"^continuous\.dat$".into();
            c.settings.regex = true;
        }
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(board, PortRole::Output, 0), port(writer, PortRole::Input, 0)).unwrap();
        graph.connect(port(writer, PortRole::Control, 0), port(command, PortRole::Setting, 0)).unwrap();

        let direct = Arc::new(DirectTriggers::default());
        let snapshot = Snapshot::new(0, graph.clone(), None);
        let runner = Runner::start_with(&snapshot, 2, direct.clone());
        let recording = Recording::prepare(&snapshot, None, chrono::Local::now(), &direct);
        let recording = snapshot.with_recording(1, Some(Arc::new(recording)));
        runner.update(&recording);
        let deadline = Instant::now() + Duration::from_secs(10);
        while runner.clock() < 0.5 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(direct.take().is_empty(), "still recording");
        // Stopped by stopping the run, as Play does: there may be no more passes.
        drop(runner);
        crate::background::wait();

        let waiting = direct.take();
        assert_eq!(waiting.len(), 1, "once the files are finished");
        let (to, Triggered::Trigger(trigger)) = waiting.into_iter().next().unwrap() else { panic!() };
        assert_eq!((to, trigger.key()), (command, crate::TriggerKind::RunOnFiles.into()));
        let Some(Nodes::RunCommand(node)) = graph.get_mut(command).map(|e| &mut e.node) else { panic!() };
        let job = send(node.clone(), trigger.take::<crate::Trigger>().unwrap(), &ControlTriggers::none());
        wait_for_job(&job);
        assert_eq!(job.problem(), None);
        let said_text = std::fs::read_to_string(&said).unwrap();
        let lines: Vec<&str> = said_text.lines().collect();
        assert_eq!(lines.len(), 2, "one file matches: {lines:?}");
        assert!(lines[0].ends_with("continuous.dat"), "{lines:?}");
        let size = std::fs::metadata(lines[0]).unwrap().len();
        assert!(size > 0 && lines[1].trim() == size.to_string(), "the whole file, closed: {size}, {lines:?}");
        std::fs::remove_file(said).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A harness showing a Run Command's window, with a writer connected to it, if `writer`, and what happened to it.
    fn window_harness(writer: bool) -> egui_kittest::Harness<'static, Graph<Nodes>> {
        window_harness_with(writer, false)
    }

    /// With the results of a run that's ended, if `ran`.
    fn window_harness_with(writer: bool, ran: bool) -> egui_kittest::Harness<'static, Graph<Nodes>> {
        let mut graph = Graph::<Nodes>::default();
        let add = |graph: &mut Graph<Nodes>, name: &str| {
            let node_type = NodeType::named(name).unwrap();
            graph.add(Nodes::create(node_type), [0.0, 0.0])
        };
        let id = add(&mut graph, "Run Command");
        let job = Job::default();
        if let Some(Nodes::RunCommand(node)) = graph.get_mut(id).map(|e| &mut e.node) {
            node.settings = settings("klusta", "{file} --overwrite");
            node.settings.pattern = ".set".into();
            node.settings.files = paths(&["/recordings/trial a.set"]);
            node.settings.files =
                ["a", "b", "c", "d", "e"].map(|name| PathBuf::from(format!("/recordings/trial {name}.set"))).to_vec();
            if ran {
                let mut state = job.0.lock().unwrap();
                let shown = |name: &str| format!("klusta \"/recordings/trial {name}.set\" --overwrite");
                let took = |seconds| Duration::from_secs_f64(seconds);
                let outcomes = [
                    Outcome::Ended { took: took(83.4), code: Some(0) },
                    Outcome::Ended { took: took(2.1), code: Some(2) },
                    Outcome::Ended { took: took(4.0), code: None },
                    Outcome::NotRun,
                ];
                let names = ["a", "b", "c", "d"];
                state.runs =
                    (names.iter().zip(outcomes)).map(|(n, outcome)| Progress { shown: shown(n), outcome }).collect();
                state.settings = Some(node.settings.clone());
            } else if !writer {
                job.fail("Couldn't open a terminal: there isn't one installed");
            }
        }
        if writer {
            let writer = add(&mut graph, "Write Raw");
            let from = Port { node: writer, role: PortRole::Control, index: 0 };
            graph.connect(from, Port { node: id, role: PortRole::Setting, index: 0 }).unwrap();
        }
        let inputs: [Option<DataStream>; 0] = [];
        // Kept, as the app keeps it.
        let state: crate::engine::UiStateCell =
            std::cell::RefCell::new(Box::new(CommandState { job, ..Default::default() }));
        let mut harness = egui_kittest::Harness::builder().with_size([560.0, 580.0]).wgpu().build_ui_state(
            move |ui, graph: &mut Graph<Nodes>| {
                let state = Some(&state);
                graph.edit(id, &inputs, &THEMES[0].colours, None, state, None, false, false, None, |node, context| {
                    node.gui_ui(ui, context)
                });
            },
            graph,
        );
        THEMES[0].apply(&harness.ctx);
        harness.run_steps(3);
        harness
    }

    fn disabled(harness: &egui_kittest::Harness<'_, Graph<Nodes>>, label: &str) -> bool {
        use egui_kittest::kittest::{NodeT, Queryable};
        harness.get_by_label(label).accesskit_node().is_disabled()
    }

    #[test]
    fn window() {
        let mut harness = window_harness(false);
        assert!(!disabled(&harness, "Run") && !disabled(&harness, "Select files…"));
        assert!(harness.query_by_label("Commands (5)").is_some());
        assert!(harness.query_by_label("Delete file 5").is_none(), "only a few files show at once");
        assert!(harness.query_by_label("5 files").is_some(), "how many there are");
        assert!(harness.query_by_label("matches 5/5").is_some(), "and how many match");
        harness.get_by_label("Examples").hover();
        harness.run_steps(4);
        let cursor = harness.output().platform_output.cursor_icon;
        assert_eq!(cursor, egui::CursorIcon::PointingHand, "help, as it is everywhere");
        harness.snapshot("run_command_examples");
        // Off it, it goes: it covers the toggle.
        harness.get_by_label("Commands (5)").hover();
        harness.run_steps(4);
        assert!(harness.query_by_label_contains("A Python script").is_none());
        harness.snapshot("run_command");
        use egui_kittest::kittest::Queryable;
        assert!(harness.query_by_label("Cheat sheet").is_none(), "only for a regular expression");
        harness.get_by_label(".*").click();
        harness.run_steps(2);
        assert!(harness.state().iter().any(|(_, e)| matches!(&e.node, Nodes::RunCommand(n) if n.settings.regex)));
        harness.get_by_label("Cheat sheet").hover();
        harness.run_steps(4);
        assert!(harness.query_by_label_contains("Rust regex syntax").is_some(), "which kind, to look up");
        assert_eq!(harness.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
        harness.snapshot("run_command_regex_cheat_sheet");
    }

    #[test]
    fn it_shows_how_each_run_went() {
        let mut harness = window_harness_with(false, true);
        use egui_kittest::kittest::Queryable;
        assert!(harness.query_by_label("Commands (4) · last run").is_some());
        harness.snapshot("run_command_last_run");
    }

    /// When to run what's after it is only asked with something connected after it. See tests/snapshots/.
    #[test]
    fn when_what_runs_after_runs_is_only_asked_with_something_after_it() {
        use egui_kittest::kittest::Queryable;
        let then_shown = |chained: bool| {
            let mut graph = Graph::<Nodes>::default();
            let node_type = NodeType::RunCommand;
            let [id, after] = [0, 1].map(|_| graph.add(Nodes::create(node_type), [0.0, 0.0]));
            if let Some(Nodes::RunCommand(node)) = graph.get_mut(id).map(|e| &mut e.node) {
                node.settings = settings("klusta", "{file} --overwrite");
                node.settings.files = paths(&["/recordings/trial a.set"]);
            }
            if chained {
                let from = Port { node: id, role: PortRole::Control, index: AFTER_RUNNING };
                graph.connect(from, Port { node: after, role: PortRole::Setting, index: 0 }).unwrap();
            }
            let inputs: [Option<DataStream>; 0] = [];
            let mut harness = egui_kittest::Harness::builder().with_size([560.0, 560.0]).wgpu().build_ui_state(
                move |ui, graph: &mut Graph<Nodes>| {
                    graph.edit(
                        id,
                        &inputs,
                        &THEMES[0].colours,
                        None,
                        None,
                        None,
                        false,
                        false,
                        None,
                        |node, context| node.gui_ui(ui, context),
                    );
                },
                graph,
            );
            THEMES[0].apply(&harness.ctx);
            harness.run_steps(3);
            let shown = harness.query_by_label("Then").is_some();
            if chained {
                harness.snapshot("run_command_then");
            }
            shown
        };
        assert!(!then_shown(false));
        assert!(then_shown(true));
    }

    #[test]
    fn a_connected_writer_is_all_it_runs_on() {
        let mut harness = window_harness(true);
        use egui_kittest::kittest::Queryable;
        assert!(disabled(&harness, "Run"));
        assert!(harness.query_by_label_contains("Supplied by Write Raw").is_some());
        assert!(harness.query_by_label("Select files…").is_none(), "what's picked isn't shown, being no use");
        let files = |harness: &egui_kittest::Harness<'_, Graph<Nodes>>| {
            let nodes = harness.state().iter().filter_map(|(_, e)| match &e.node {
                Nodes::RunCommand(node) => Some(node.settings.files.len()),
                _ => None,
            });
            nodes.sum::<usize>()
        };
        assert_eq!(files(&harness), 5, "but it's kept, for when it's not connected");
        assert!(harness.query_by_label("Run triggered by Write Raw").is_some());
        assert!(!disabled(&harness, "Browse…"), "what to run's still up to you");
        harness.snapshot("run_command_connected");
    }
}

/// What runs after it, and loops of what runs after what, which can be checked anywhere.
#[cfg(test)]
mod chain_tests {
    use std::sync::Arc;

    use super::*;
    use crate::NodeType;
    use crate::Nodes;
    use crate::engine::{Graph, NodeId, NodesEnum, Port, PortRole};
    use crate::triggers::{DirectTriggers, Triggered};

    /// Whether `then`, after runs that ended with these codes (`None` if stopped; not there if not run), sends the
    /// files on to what's connected after it.
    fn sends(then: Then, runs: &[Option<Option<i64>>]) -> bool {
        let direct = Arc::new(DirectTriggers::default());
        let kinds: &'static [TriggerKind] = &[TriggerKind::RunOnFiles];
        let triggers = ControlTriggers::<Trigger>::new(direct.clone(), vec![vec![(NodeId(2), kinds)]]);
        let handle = triggers.handle(AFTER_RUNNING, TriggerKind::RunOnFiles).unwrap();
        let progress = runs.iter().map(|run| Progress {
            shown: String::new(),
            outcome: match run {
                Some(code) => Outcome::Ended { took: Duration::ZERO, code: *code },
                None => Outcome::NotRun,
            },
        });
        Chain { handle, then, files: vec![PathBuf::from("a.dat")] }.send_after(&progress.collect::<Vec<_>>());
        let sent: Vec<(NodeId, Option<Trigger>)> = direct
            .take()
            .into_iter()
            .map(|(node, triggered)| match triggered {
                Triggered::Trigger(trigger) => (node, trigger.take::<Trigger>()),
                Triggered::SettingsUpdate { .. } => panic!("not a settings update"),
            })
            .collect();
        match &sent[..] {
            [] => false,
            [(NodeId(2), Some(Trigger::RunOnFiles(RunOnFiles(files))))] => {
                assert_eq!(files, &[PathBuf::from("a.dat")], "the files it ran on");
                true
            }
            _ => panic!("sent once, to what's connected"),
        }
    }

    #[test]
    fn what_runs_after_is_sent_the_files_as_then_says_and_never_once_stopped() {
        let thens = [Then::OnSuccess, Then::OnFailure, Then::Always];
        let (ok, failed) = ([Some(Some(0)), Some(Some(0))], [Some(Some(0)), Some(Some(2))]);
        assert_eq!(thens.map(|then| sends(then, &ok)), [true, false, true]);
        assert_eq!(thens.map(|then| sends(then, &failed)), [false, true, true]);
        let (stopped, not_run) = ([Some(Some(1)), Some(None)], [Some(Some(0)), None]);
        assert_eq!(thens.map(|then| sends(then, &stopped)), [false; 3]);
        assert_eq!(thens.map(|then| sends(then, &not_run)), [false; 3]);
    }

    #[test]
    fn commands_running_after_each_other_round_a_loop_are_warned_of() {
        let mut graph = Graph::<Nodes>::default();
        let node_type = NodeType::RunCommand;
        let [a, b, c] = [0, 1, 2].map(|_| graph.add(Nodes::create(node_type), [0.0, 0.0]));
        let after = |from, to| {
            let control = Port { node: from, role: PortRole::Control, index: AFTER_RUNNING };
            (control, Port { node: to, role: PortRole::Setting, index: 0 })
        };
        let looped = |graph: &Graph<Nodes>| {
            let resolved = graph.resolve();
            [a, b, c].map(|id| resolved[&id].warnings.iter().any(|w| w.contains("loop of triggers")))
        };
        for (from, to) in [after(a, b), after(b, c)] {
            graph.connect(from, to).unwrap();
        }
        assert_eq!(looped(&graph), [false; 3], "one after another");
        let (from, to) = after(c, a);
        graph.connect(from, to).unwrap();
        assert_eq!(looped(&graph), [true; 3], "and round again");
        let edge = graph.edge_into(to).unwrap();
        graph.set_edge_disabled(edge, true);
        assert_eq!(looped(&graph), [false; 3], "not with an edge of it disabled");
    }
}
