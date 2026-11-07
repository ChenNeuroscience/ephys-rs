//! Runs a program on files, in a terminal of its own, whatever the graph's doing: by hand, on files picked here, or by
//! itself, on files sent to it with `RUN_ON_FILES` (by a writer, once it's finished recording them, say). It's nothing
//! to do with passes: the files come as a direct trigger (see `crate::core::triggers`).
//!
//! The terminal's a process apart, running a script written here, so its output's there, not here, and it carries on if
//! the app quits. To be stopped it has to be findable, and what's started here often isn't what's running (macOS's `open`
//! hands the script to Terminal and goes): see `Terminal`.
//!
//! The arguments are quoted in the script, so a path with spaces in it arrives whole, but pipes and `&&` mean nothing,
//! unless the program's a shell (`bash`, with `-c "..."`).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::core::engine::{Category, Context, EdgeInfo, EdgeType, Node, NodeInfo, Output, SettingPort};
use crate::core::gui::widgets::{Icon, help_link, small_icon_button};
use crate::core::theme::Colours;
use crate::standard_components::recording::expand;
use crate::standard_components::widgets::{combo, setting, settings_grid};
use crate::{DataStream, Edges, Setting};

/// The direct trigger of a command setting (see `crate::core::triggers`): run it on the files sent with it, a
/// `Vec<PathBuf>`. Whatever sends them, and whenever: a writer's are what it's just finished recording.
pub const RUN_ON_FILES: &str = "run on files";

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

/// `text`, as a single word to a shell.
#[cfg(any(unix, test))]
fn shell_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// A script running `runs` from `folder`, in turn, or all at once if `together`, then waiting for return, so what they
/// said can be read. It deletes itself, keeps its process's ID in `pid_file` until it ends, however it ends, and adds a
/// line to `status_file` as each run starts and ends: `start 0`, then `end 0 3`, with the exit code.
#[cfg(any(unix, test))]
fn shell_script(folder: &Path, pid_file: &Path, status_file: &Path, runs: &[Run], how: How) -> String {
    let pid_file = shell_quoted(&pid_file.to_string_lossy());
    let status_file = shell_quoted(&status_file.to_string_lossy());
    let remove_pid_file = shell_quoted(&format!("rm -f -- {pid_file}"));
    let folder = shell_quoted(&folder.to_string_lossy());
    let mut script = format!(
        "#!/bin/sh\nrm -f -- \"$0\"\necho $$ > {pid_file}\ntrap {remove_pid_file} EXIT\ntrap exit HUP INT TERM\ncd {folder}\n"
    );
    for (index, run) in runs.iter().enumerate() {
        let mut words = vec![shell_quoted(&run.program.to_string_lossy())];
        words.extend(run.arguments.iter().map(|a| shell_quoted(a)));
        let shown = shell_quoted(&run.shown());
        let body = format!(
            "printf '$ %s\\n' {shown}\necho 'start {index}' >> {status_file}\n{}\nc=$?\necho \"end {index} $c\" >> \
             {status_file}\n[ \"$c\" -eq 0 ] || printf 'Failed, exit status %s: %s\\n' \"$c\" {shown}\n",
            words.join(" ")
        );
        // Together, each in the background: still the script's process group, so stopped with it.
        script += &if how.together { format!("(\n{body}) &\n") } else { body };
    }
    if how.together {
        script += "wait\n";
    }
    // Something failed if an end has an exit code other than 0.
    let wait = "printf '\\nFinished: press return to close. '\nread -r _\n";
    if how.close {
        let close_window = if how.close_window { CLOSE_WINDOW } else { "" };
        script +=
            &format!("if grep -Eq '^end [0-9]+ ([^0]|0.)' {status_file}; then\n{wait}else\n:\n{close_window}fi\n");
    } else {
        script += wait;
    }
    script + "exit 0\n"
}

/// `text`, as a single word on a line of a batch file run with `call`. `call` reads a line twice, so a `%` needs doubling
/// twice.
#[cfg(any(windows, test))]
fn batch_quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\\\"").replace('%', "%%%%"))
}

/// A batch file running `runs` from `folder`, in turn, or all at once if `together`, then waiting for a key, so what
/// they said can be read. It deletes itself, and adds a line to `status_file` as each run starts and ends, as
/// `shell_script` does. Each run's a part of its own, which the file calls, in turn, or starts copies of itself to go
/// straight to, together, waiting till the status file says they've all ended. Lines without an `@` are shown as they
/// run. `call`, as a program that's a batch file wouldn't come back otherwise.
#[cfg(any(windows, test))]
fn batch_script(folder: &Path, status_file: &Path, runs: &[Run], together: bool, close: bool) -> String {
    let folder = folder.to_string_lossy().replace('%', "%%");
    let status_file = status_file.to_string_lossy().replace('%', "%%");
    let mut script = format!("@chcp 65001 >nul\r\n@cd /d \"{folder}\"\r\n@if not \"%~1\"==\"\" goto run%~1\r\n");
    for index in 0..runs.len() {
        script += &if together {
            format!("@start \"\" /b cmd /c call \"%~f0\" {index}\r\n")
        } else {
            format!("@call :run{index}\r\n")
        };
    }
    if together {
        let count = runs.len();
        script += &format!(
            ":waiting\r\n@ping -n 2 127.0.0.1 >nul\r\n@for /f %%n in ('type \"{status_file}\" ^| find /c \"end \"') do \
             @if %%n LSS {count} goto waiting\r\n"
        );
    }
    if close {
        // Something failed if an end has an exit code other than 0.
        script += &format!("@findstr /r /c:\"^end [0-9]* [^0]\" \"{status_file}\" >nul || goto finished\r\n");
    }
    script += "@echo.\r\n@echo Finished.\r\n@pause\r\n:finished\r\n@(goto) 2>nul & del \"%~f0\"\r\n";
    for (index, run) in runs.iter().enumerate() {
        let mut words = vec![batch_quoted(&run.program.to_string_lossy())];
        words.extend(run.arguments.iter().map(|a| batch_quoted(a)));
        // The redirection first: `echo start 0>>` would be of handle 0.
        script += &format!(
            ":run{index}\r\n@>>\"{status_file}\" echo start {index}\r\ncall {}\r\n@if not \"%errorlevel%\"==\"0\" echo \
             Failed, exit code %errorlevel%\r\n@>>\"{status_file}\" echo end {index} %errorlevel%\r\n@exit /b\r\n",
            words.join(" ")
        );
    }
    script
}

/// Somewhere in the temporary folder for a script, with `extension`: unused, by this app or any other running.
fn temporary_path(extension: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("ephys-rs-run-{}-{number}.{extension}", std::process::id()))
}

/// Writes a file only this user can get at, that isn't there already: the temporary folder may be everyone's.
#[cfg(unix)]
fn write_private(path: &Path, text: &str, mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(path)?.write_all(text.as_bytes())
}

/// An empty file for a script to write to, only this user can get at, where possible.
fn create_private(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return write_private(path, "", 0o600);
    #[cfg(not(unix))]
    std::fs::write(path, "")
}

/// What might open a terminal on `script`, best first. `open` goes by what's set to open `.command` files: Terminal,
/// unless that's been changed.
#[cfg(all(target_os = "macos", not(test)))]
fn terminals(script: &Path) -> Vec<Command> {
    let mut open = Command::new("open");
    open.arg(script);
    vec![open]
}

/// What might open a terminal on `script`, best first.
#[cfg(all(unix, not(target_os = "macos"), not(test)))]
fn terminals(script: &Path) -> Vec<Command> {
    let ways = [
        ("x-terminal-emulator", "-e"),
        ("gnome-terminal", "--"),
        ("konsole", "-e"),
        ("xfce4-terminal", "-x"),
        ("xterm", "-e"),
    ];
    let terminal = |(name, run): (&str, &str)| {
        let mut terminal = Command::new(name);
        terminal.arg(run).arg(script);
        terminal
    };
    ways.into_iter().map(terminal).collect()
}

/// In tests, no windows: the script, in a process group of its own, as it would be in a terminal.
#[cfg(all(unix, test))]
fn terminals(script: &Path) -> Vec<Command> {
    use std::os::unix::process::CommandExt;
    let mut shell = Command::new("/bin/sh");
    shell.arg(script).process_group(0);
    vec![shell]
}

/// A script's process, in a terminal, which may have ended. On Windows, the terminal's a child of this process.
/// Elsewhere it may not be, so the script leaves word in a file: it's going while the file's there, and the file says
/// what to stop.
#[cfg(unix)]
struct Process {
    pid_file: PathBuf,
}

#[cfg(unix)]
impl Process {
    /// `job`'s told if the terminal doesn't manage to start.
    fn open(job: &Job, folder: &Path, status_file: &Path, runs: &[Run], how: How) -> Result<Process, String> {
        let (script, pid_file) = (temporary_path("command"), temporary_path("pid"));
        let how = How { close_window: cfg!(target_os = "macos"), ..how };
        let text = shell_script(folder, &pid_file, status_file, runs, how);
        let written = write_private(&script, &text, 0o700).and_then(|()| write_private(&pid_file, "", 0o600));
        let remove = {
            let (script, pid_file) = (script.clone(), pid_file.clone());
            move || {
                let _ = std::fs::remove_file(&script);
                let _ = std::fs::remove_file(&pid_file);
            }
        };
        if let Err(e) = written {
            remove();
            return Err(format!("Couldn't write a script to run: {e}"));
        }
        let mut problem = "there isn't one installed".to_string();
        for mut terminal in terminals(&script) {
            match terminal.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                Ok(child) => {
                    Self::watch(job.clone(), child, pid_file.clone(), remove);
                    return Ok(Process { pid_file });
                }
                Err(e) => problem = e.to_string(),
            }
        }
        remove();
        Err(format!("Couldn't open a terminal: {problem}"))
    }

    /// Waits for what opened the terminal, which may well end first, on a thread nothing else waits for: some last as
    /// long as the terminal, which needn't stop the app quitting. If it failed with the script not started, it won't.
    fn watch(job: Job, mut child: Child, pid_file: PathBuf, remove: impl FnOnce() + Send + 'static) {
        let watch = move || {
            let failed = child.wait().is_ok_and(|status| !status.success());
            let started = std::fs::read_to_string(&pid_file).map_or(true, |pid| !pid.trim().is_empty());
            if failed && !started {
                remove();
                job.fail("Couldn't open a terminal");
            }
        };
        let _ = std::thread::Builder::new().name("run command terminal".into()).spawn(watch);
    }

    fn going(&mut self) -> bool {
        self.pid_file.exists()
    }

    /// Stops the script and what it's running: the shell it's in gave it a process group of its own.
    fn stop(&mut self) {
        let pid = std::fs::read_to_string(&self.pid_file).unwrap_or_default();
        let pid = pid.trim();
        let group = format!("-{pid}");
        let stopped = !pid.is_empty()
            && Command::new("kill")
                .args(["-TERM", "--", &group])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
        if !stopped {
            // Not started, or gone without saying so.
            let _ = std::fs::remove_file(&self.pid_file);
        }
    }
}

/// A script's process, in a terminal, which may have ended. On Windows, the terminal's a child of this process.
/// Elsewhere it may not be, so the script leaves word in a file: it's going while the file's there, and the file says
/// what to stop.
#[cfg(windows)]
struct Process {
    child: Child,
}

#[cfg(windows)]
impl Process {
    fn open(_job: &Job, folder: &Path, status_file: &Path, runs: &[Run], how: How) -> Result<Process, String> {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x10;
        let script = temporary_path("bat");
        std::fs::write(&script, batch_script(folder, status_file, runs, how.together, how.close))
            .map_err(|e| format!("Couldn't write a script to run: {e}"))?;
        let child = Command::new("cmd").arg("/C").arg(&script).creation_flags(CREATE_NEW_CONSOLE).spawn();
        child.map(|child| Process { child }).map_err(|e| {
            let _ = std::fs::remove_file(&script);
            format!("Couldn't open a terminal: {e}")
        })
    }

    fn going(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Stops the terminal and everything it started: `Child::kill` would leave the program.
    fn stop(&mut self) {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let pid = self.child.id().to_string();
        let stopped = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        if !stopped.is_ok_and(|status| status.success()) {
            let _ = self.child.kill();
        }
    }
}

/// How a lot's run in a terminal: all at once, or in turn; and whether the terminal's left to close if they all succeed.
#[derive(Clone, Copy, Default)]
struct How {
    together: bool,
    close: bool,
    /// Whether the terminal has to be told to close the window, as macOS's Terminal does, unless set otherwise.
    #[cfg_attr(windows, allow(dead_code))]
    close_window: bool,
}

/// Has macOS's Terminal close the window the script's in, once it's ended (or it'd ask whether to stop it), on a
/// process apart (the window's going). Only in a terminal: not in tests, say.
#[cfg(any(unix, test))]
const CLOSE_WINDOW: &str = r#"tty=$(tty)
case $tty in /dev/*)
( trap '' HUP; sleep 0.5; osascript -e 'on run argv' -e 'tell application "Terminal"' -e 'repeat with w in windows' \
-e 'repeat with t in tabs of w' -e 'if tty of t is item 1 of argv then close w' -e 'end repeat' -e 'end repeat' \
-e 'end tell' -e 'end run' "$tty" ) < /dev/null > /dev/null 2>&1 &
esac
"#;

/// How one run's going, or went.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Outcome {
    Waiting,
    Running(Instant),
    /// With its exit code, unless it was stopped.
    Ended {
        took: Duration,
        code: Option<i64>,
    },
    /// Stopped before it started.
    NotRun,
}

/// One run of a lot, as it might be typed, and how it's going.
#[derive(Clone, PartialEq, Debug)]
struct Progress {
    shown: String,
    outcome: Outcome,
}

/// A script running in a terminal, which may have ended, and what it's said of how its runs are going.
struct Terminal {
    process: Process,
    /// Where the script says as each run starts and ends.
    status_file: PathBuf,
    /// The lines of it taken in already.
    seen: usize,
}

impl Terminal {
    fn open(job: &Job, runs: &[Run], how: How) -> Result<Terminal, String> {
        let folder = std::env::current_dir().map_err(|e| e.to_string())?;
        let status_file = temporary_path("status");
        create_private(&status_file).map_err(|e| format!("Couldn't write a script to run: {e}"))?;
        match Process::open(job, &folder, &status_file, runs, how) {
            Ok(process) => Ok(Terminal { process, status_file, seen: 0 }),
            Err(problem) => {
                let _ = std::fs::remove_file(&status_file);
                Err(problem)
            }
        }
    }

    /// Takes in what the script's said since the last time, of `runs`, as of `now`.
    fn update(&mut self, runs: &mut [Progress], now: Instant) {
        let Ok(text) = std::fs::read_to_string(&self.status_file) else { return };
        // A line's only whole once it's ended.
        let lines: Vec<&str> = text.split_inclusive('\n').filter(|line| line.ends_with('\n')).collect();
        for line in lines.iter().skip(self.seen) {
            let words: Vec<&str> = line.split_whitespace().collect();
            let Some(progress) = words.get(1).and_then(|i| i.parse::<usize>().ok()).and_then(|i| runs.get_mut(i))
            else {
                continue;
            };
            match (words[0], progress.outcome) {
                ("start", _) => progress.outcome = Outcome::Running(now),
                ("end", Outcome::Running(started)) => {
                    let code = words.get(2).and_then(|code| code.parse().ok());
                    progress.outcome = Outcome::Ended { took: now - started, code: Some(code.unwrap_or(-1)) };
                }
                _ => {}
            }
        }
        self.seen = self.seen.max(lines.len());
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.status_file);
    }
}

/// How it's going.
#[derive(Default)]
struct JobState {
    /// The terminal it's running a lot in, if it is: one at a time.
    terminal: Option<Terminal>,
    /// The runs of that lot, or the last, and the settings they were of: they're shown until those change.
    runs: Vec<Progress>,
    settings: Option<CommandSettings>,
    /// What went wrong the last time it tried to run.
    problem: Option<String>,
}

/// What's been run. Not a setting: every copy of the node (in undo's history, say) has the same one, and they're equal.
#[derive(Clone, Default)]
pub struct Job(Arc<Mutex<JobState>>);

impl PartialEq for Job {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Job")
    }
}

impl Job {
    /// Says what's wrong, in the node's window and the log: it may have been nobody's doing, after a recording.
    fn fail(&self, problem: impl Into<String>) {
        let problem = problem.into();
        log::error!("Run Command: {problem}");
        self.0.lock().unwrap().problem = Some(problem);
    }

    fn problem(&self) -> Option<String> {
        self.0.lock().unwrap().problem.clone()
    }

    fn running(&self) -> bool {
        self.0.lock().unwrap().terminal.is_some()
    }

    /// The runs of the lot going, or of the last, if that was with `settings`.
    fn runs_with(&self, settings: &CommandSettings) -> Option<Vec<Progress>> {
        let state = self.0.lock().unwrap();
        (state.terminal.is_some() || state.settings.as_ref() == Some(settings)).then(|| state.runs.clone())
    }

    /// Runs `runs`, made with `settings`, in turn in a new terminal, unless it's running already.
    fn start(&self, settings: &CommandSettings, runs: &[Run]) {
        {
            let mut state = self.0.lock().unwrap();
            if state.terminal.is_some() {
                drop(state);
                return self.fail("It was still running, so it didn't run on what was sent");
            }
            state.problem = None;
            state.runs = runs.iter().map(|run| Progress { shown: run.shown(), outcome: Outcome::Waiting }).collect();
            state.settings = Some(settings.clone());
        }
        let how =
            How { together: settings.mode == Mode::EachTogether, close: settings.close_on_success, ..How::default() };
        match Terminal::open(self, runs, how) {
            Ok(terminal) => {
                self.0.lock().unwrap().terminal = Some(terminal);
                self.watch();
            }
            Err(problem) => {
                let mut state = self.0.lock().unwrap();
                (state.runs, state.settings) = (Vec::new(), None);
                drop(state);
                self.fail(problem);
            }
        }
    }

    /// Follows the terminal, on a thread of its own, till it ends: it's timed here, so it needn't be shown.
    fn watch(&self) {
        let job = self.clone();
        let watch = move || {
            loop {
                std::thread::sleep(Duration::from_millis(50));
                let mut state = job.0.lock().unwrap();
                let JobState { terminal, runs, .. } = &mut *state;
                let Some(open) = terminal else { return };
                let now = Instant::now();
                open.update(runs, now);
                if !open.process.going() {
                    open.update(runs, now);
                    for progress in runs.iter_mut() {
                        progress.outcome = match progress.outcome {
                            Outcome::Running(started) => Outcome::Ended { took: now - started, code: None },
                            Outcome::Waiting => Outcome::NotRun,
                            ended => ended,
                        };
                    }
                    *terminal = None;
                    return;
                }
            }
        };
        if let Err(e) = std::thread::Builder::new().name("run command progress".into()).spawn(watch) {
            self.0.lock().unwrap().terminal = None;
            self.fail(format!("Couldn't follow how it's going: {e}"));
        }
    }

    /// Stops what's running in its terminal, and what's waiting there to run.
    fn stop(&self) {
        if let Some(terminal) = self.0.lock().unwrap().terminal.as_mut() {
            terminal.process.stop();
        }
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
/// can change. Not a setting, like `Job`.
#[derive(Clone, Default)]
pub struct PreviewCache(Arc<Mutex<Option<Arc<Preview>>>>);

impl PartialEq for PreviewCache {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for PreviewCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreviewCache")
    }
}

impl PreviewCache {
    /// What `settings` come to, with an example file in place of those sent, if something that sends them is
    /// `connected`.
    fn of(&self, settings: &CommandSettings, connected: bool) -> Arc<Preview> {
        let mut kept = self.0.lock().unwrap();
        if let Some(preview) = kept.as_ref()
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
        *kept = Some(preview.clone());
        preview
    }
}

/// Runs a program on files.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunCommand {
    pub settings: CommandSettings,
    #[serde(skip)]
    job: Job,
    #[serde(skip)]
    preview: PreviewCache,
}

impl RunCommand {
    /// Runs the program on `files`, or says why it can't.
    fn run_on(&self, files: Result<Vec<PathBuf>, String>) {
        match files.and_then(|files| self.settings.runs(&files)) {
            Ok(runs) => self.job.start(&self.settings, &runs),
            Err(problem) => self.job.fail(problem),
        }
    }

    fn window(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        ui.horizontal(|ui| context.node_toolbar(ui));
        ui.horizontal(|ui| {
            ui.label("Runs a program on files, in a terminal window of its own.");
            help_link(ui, "Examples", examples);
        });
        let warning = "Recording performance may suffer if the computer's resources are strained by other running \
                       commands.";
        ui.label(RichText::new(warning).color(context.theme.warning));
        ui.add_space(6.0);
        let job = self.job.clone();
        let (locked, running) = (context.locked, job.running());
        // What's connected supplies all the files, while it is.
        let senders = context.controlled_by.first().map_or(&[][..], Vec::as_slice);
        let connected = !senders.is_empty();
        let preview = self.preview.of(&self.settings, connected);
        // Nothing changes while it's running, to what it shows as running.
        // How many files picked here match, of how many.
        let matched = preview.picked.as_ref().ok().map(Vec::len);
        let counts = preview.listed.zip(matched).map(|(listed, matched)| (matched, listed));
        self.settings_section(ui, senders, counts, locked || running);
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
                if run.clicked() {
                    self.run_on(picked.clone());
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
            });
        });
    }
}

/// A run of a lot, with how it went, or is going: a mark, how long it took, and the exit code if it failed.
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

    const INFO: NodeInfo = NodeInfo {
        name: "Run Command",
        category: Category::Other,
        description: "Runs a program on files, in a terminal: by hand, or on files sent to it (by a writer, after recording).",
    };

    const HAS_VIEW: bool = true;
    const SHOW_IN_BASIC_DEFAULT: bool = true;

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        vec![SettingPort { name: "command", value: self.settings.clone().into(), on_right: false }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Command(settings) = value {
            self.settings = settings;
        }
    }

    fn direct_triggers(&self, _setting: usize) -> &'static [&'static str] {
        &[RUN_ON_FILES]
    }

    /// Runs the program on what's been recorded.
    fn on_trigger(&mut self, name: &'static str, payload: Box<dyn std::any::Any + Send>) {
        if name == RUN_ON_FILES
            && let Ok(paths) = payload.downcast::<Vec<PathBuf>>()
        {
            self.run_on(self.settings.matching(*paths));
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        self.window(ui, context);
    }

    fn view_ui(&mut self, ui: &mut egui::Ui, context: &mut Context<Edges>) {
        self.window(ui, context);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use super::*;
    use crate::Nodes;
    use crate::core::engine::{Graph, NodeSet, Port, Recording, Role, Runner, Snapshot};
    use crate::core::triggers::{DirectTriggers, Triggered};
    use crate::standard_components::ephys::board::{Source, serve_test_board};
    use crate::standard_components::recording::Location;
    use crate::standard_components::themes::THEMES;

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

    fn wait_for_job(job: &Job) {
        wait_until("it should have finished", || !job.running());
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
        job.start(&CommandSettings::default(), &runs);
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
        job.start(&command, &command.runs(&paths(&["0", "3"])).unwrap());
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
        job.start(&command, &runs);
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
        let cache = PreviewCache::default();
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
        let mut node = RunCommand::default();
        node.run_on(Ok(Vec::new()));
        assert_eq!(node.job.problem().as_deref(), Some("There's no program to run"));
        node.settings = settings("/usr/bin/true", "");
        node.run_on(Ok(Vec::new()));
        assert_eq!(node.job.problem(), None);
        wait_for_job(&node.job);
    }

    #[test]
    fn it_is_not_a_change_to_the_graph() {
        let node = RunCommand::default();
        let copy = node.clone();
        node.job.fail("something");
        assert_eq!(node, copy);
        let saved = serde_json::to_value(&node).unwrap();
        assert!(saved.get("job").is_none());
    }

    /// Real passes and a real recording: a writer's files go to what's connected to it, once they're finished, and only
    /// those that match.
    #[test]
    fn a_writer_has_it_run_on_what_it_recorded() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-after-recording-{}", std::process::id()));
        let said = std::env::temp_dir().join(format!("ephys-rs-after-recording-{}.txt", std::process::id()));
        let add = |graph: &mut Graph<Nodes>, name: &str| {
            let index = Nodes::TYPES.iter().position(|t| t.name == name).unwrap();
            graph.add(Nodes::create(index), [0.0, 0.0])
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
        graph.connect(port(board, Role::Output, 0), port(writer, Role::Input, 0)).unwrap();
        graph.connect(port(writer, Role::Control, 0), port(command, Role::Setting, 0)).unwrap();

        let direct = Arc::new(DirectTriggers::default());
        let snapshot = Snapshot::new(0, graph.clone(), None);
        let runner = Runner::start_with(&snapshot, 2, direct.clone());
        let recording = Recording::prepare(&snapshot, None, &direct);
        let recording = snapshot.with_recording(1, Some(Arc::new(recording)));
        runner.update(&recording);
        let deadline = Instant::now() + Duration::from_secs(10);
        while runner.clock() < 0.5 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(direct.take().is_empty(), "still recording");
        // Stopped by stopping the run, as Play does: there may be no more passes.
        drop(runner);
        crate::core::background::wait();

        let waiting = direct.take();
        assert_eq!(waiting.len(), 1, "once the files are finished");
        let (to, Triggered::Named { name, payload }) = waiting.into_iter().next().unwrap() else { panic!() };
        assert_eq!((to, name), (command, RUN_ON_FILES));
        let Some(Nodes::RunCommand(node)) = graph.get_mut(command).map(|e| &mut e.node) else { panic!() };
        Node::on_trigger(node, name, payload);
        wait_for_job(&node.job);
        assert_eq!(node.job.problem(), None);
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
            let index = Nodes::TYPES.iter().position(|t| t.name == name).unwrap();
            graph.add(Nodes::create(index), [0.0, 0.0])
        };
        let id = add(&mut graph, "Run Command");
        if let Some(Nodes::RunCommand(node)) = graph.get_mut(id).map(|e| &mut e.node) {
            node.settings = settings("klusta", "{file} --overwrite");
            node.settings.pattern = ".set".into();
            node.settings.files = paths(&["/recordings/trial a.set"]);
            node.settings.files =
                ["a", "b", "c", "d", "e"].map(|name| PathBuf::from(format!("/recordings/trial {name}.set"))).to_vec();
            if ran {
                let mut state = node.job.0.lock().unwrap();
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
                node.job.fail("Couldn't open a terminal: there isn't one installed");
            }
        }
        if writer {
            let writer = add(&mut graph, "Write Raw");
            let from = Port { node: writer, role: Role::Control, index: 0 };
            graph.connect(from, Port { node: id, role: Role::Setting, index: 0 }).unwrap();
        }
        let inputs: [Option<DataStream>; 0] = [];
        let mut harness = egui_kittest::Harness::builder().with_size([560.0, 580.0]).wgpu().build_ui_state(
            move |ui, graph: &mut Graph<Nodes>| {
                graph.edit(id, &inputs, &THEMES[0].colours, None, None, false, false, None, |node, context| {
                    node.view_ui(ui, context)
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

/// What's written for each platform, which can be read anywhere, if only run on its own.
#[cfg(test)]
mod script_tests {
    use super::*;

    fn run() -> Run {
        Run { program: "C:\\Program Files\\klusta.exe".into(), arguments: vec!["100%.set".into(), "a b".into()] }
    }

    #[test]
    fn a_batch_file_quotes_what_it_runs_and_waits_at_the_end() {
        let script = batch_script(Path::new("C:\\here 100%"), Path::new("C:\\status"), &[run()], false, false);
        let lines: Vec<&str> = script.split("\r\n").collect();
        assert_eq!(lines[1], "@cd /d \"C:\\here 100%%\"");
        assert!(lines.contains(&"@call :run0"), "{script}");
        assert!(lines.contains(&"call \"C:\\Program Files\\klusta.exe\" \"100%%%%.set\" \"a b\""), "{script}");
        assert!(lines.contains(&"@>>\"C:\\status\" echo end 0 %errorlevel%"), "{script}");
        let pause = lines.iter().position(|line| *line == "@pause").unwrap();
        assert!(pause < lines.iter().position(|line| *line == ":run0").unwrap(), "runs are called, not fallen into");
    }

    #[test]
    fn a_batch_file_starts_copies_of_itself_to_run_them_together() {
        let script = batch_script(Path::new("C:\\here"), Path::new("C:\\status"), &[run(), run()], true, false);
        assert!(script.contains("@start \"\" /b cmd /c call \"%~f0\" 1\r\n"), "{script}");
        assert!(script.contains("@if %%n LSS 2 goto waiting"), "{script}");
        assert!(script.contains("goto run%~1"), "{script}");
        let closing = batch_script(Path::new("C:\\here"), Path::new("C:\\status"), &[run()], false, true);
        let lines: Vec<&str> = closing.split("\r\n").collect();
        let skip = lines.iter().position(|line| line.contains("goto finished")).unwrap();
        assert!(skip < lines.iter().position(|line| *line == "@pause").unwrap(), "{closing}");
        assert!(
            lines.iter().position(|line| *line == ":finished").unwrap()
                < lines.iter().position(|line| *line == ":run0").unwrap()
        );
    }

    #[test]
    fn a_shell_script_leaves_word_of_itself() {
        let script =
            shell_script(Path::new("/here"), Path::new("/tmp/it's.pid"), Path::new("/s"), &[run()], How::default());
        assert!(script.contains(r"echo $$ > '/tmp/it'\''s.pid'"), "{script}");
    }
}

/// Real batch files and terminals, which only Windows has.
#[cfg(all(test, windows))]
mod windows_tests {
    use std::time::Instant;

    use super::*;

    fn cmd(command: &str) -> Run {
        Run { program: "cmd".into(), arguments: vec!["/C".into(), command.into()] }
    }

    #[test]
    fn a_batch_file_says_what_programs_said_and_deletes_itself() {
        let script = temporary_path("bat");
        let status = temporary_path("status");
        let runs = [cmd("echo said 100%"), cmd("exit 3")];
        std::fs::write(&script, batch_script(&std::env::temp_dir(), &status, &runs, false, false)).unwrap();
        let output = Command::new("cmd").arg("/C").arg(&script).stdin(Stdio::null()).output().unwrap();
        let said_status = std::fs::read_to_string(&status).unwrap();
        assert!(said_status.contains("end 0 0") && said_status.contains("end 1 3"), "{said_status}");
        std::fs::remove_file(&status).unwrap();
        let said = String::from_utf8_lossy(&output.stdout);
        assert!(said.contains("said 100%"), "{said}");
        assert!(said.contains("Failed, exit code 3"), "{said}");
        assert!(said.contains("Finished"), "{said}");
        assert!(!script.exists(), "{said}");
    }

    #[test]
    fn a_batch_file_only_waits_if_something_failed() {
        for (command, waits) in [("exit 0", false), ("exit 2", true)] {
            let (script, status) = (temporary_path("bat"), temporary_path("status"));
            std::fs::write(&script, batch_script(&std::env::temp_dir(), &status, &[cmd(command)], false, true))
                .unwrap();
            let output = Command::new("cmd").arg("/C").arg(&script).stdin(Stdio::null()).output().unwrap();
            let said = String::from_utf8_lossy(&output.stdout);
            assert_eq!(said.contains("Finished."), waits, "{command}: {said}");
            std::fs::remove_file(&status).unwrap();
        }
    }

    #[test]
    fn a_batch_file_runs_them_together() {
        let (script, status) = (temporary_path("bat"), temporary_path("status"));
        let runs = [cmd("ping -n 2 127.0.0.1 >nul"), cmd("exit 4")];
        std::fs::write(&script, batch_script(&std::env::temp_dir(), &status, &runs, true, false)).unwrap();
        let output = Command::new("cmd").arg("/C").arg(&script).stdin(Stdio::null()).output().unwrap();
        let said = std::fs::read_to_string(&status).unwrap();
        assert!(
            said.contains("end 0 0") && said.contains("end 1 4"),
            "{said} {}",
            String::from_utf8_lossy(&output.stdout)
        );
        std::fs::remove_file(&status).unwrap();
    }

    #[test]
    fn a_terminal_can_be_stopped() {
        let job = Job::default();
        let ping = ["-n", "60", "127.0.0.1"].map(String::from).to_vec();
        job.start(&CommandSettings::default(), &[Run { program: "ping".into(), arguments: ping }]);
        assert_eq!(job.problem(), None);
        std::thread::sleep(Duration::from_secs(1));
        assert!(job.running());
        job.stop();
        let deadline = Instant::now() + Duration::from_secs(10);
        while job.running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!job.running());
    }
}
