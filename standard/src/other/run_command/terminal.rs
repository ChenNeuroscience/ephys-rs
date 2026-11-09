//! The terminal side of Run Command: the scripts it writes (a shell script, or a batch file on Windows), the terminals
//! it opens to run them in, and following and stopping what's running there (`Job`).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{CommandSettings, Mode, Run, RunOnFiles, Then};
use crate::Trigger;
use ephys_rs::triggers::TriggerHandle;

/// `text`, as a single word to a shell.
#[cfg(any(unix, test))]
fn shell_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// A script running `runs` from `folder`, in turn, or all at once if `together`, then waiting for return, so what they
/// said can be read. It deletes itself, keeps its process's ID in `pid_file` until it ends, however it ends, and adds a
/// line to `status_file` as each run starts and ends: `start 0`, then `end 0 3`, with the exit code.
#[cfg(any(unix, test))]
pub(super) fn shell_script(folder: &Path, pid_file: &Path, status_file: &Path, runs: &[Run], how: How) -> String {
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

/// `text`, as a single word on a line of a batch file run with `call`: in quotes only if it has to be, as some programs
/// (`cmd` itself) read their own command line, and a quoted `/C` isn't `/C` to it. `call` reads a line twice, so a `%`
/// needs doubling twice.
#[cfg(any(windows, test))]
fn batch_quoted(text: &str) -> String {
    let text = text.replace('%', "%%%%");
    let special = |c: char| c.is_whitespace() || "\"&|<>^(),;=".contains(c);
    if !text.is_empty() && !text.chars().any(special) {
        return text;
    }
    format!("\"{}\"", text.replace('"', "\\\""))
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
        // Each run's end, in turn: `findstr` says if it's not there yet.
        script += ":waiting\r\n@ping -n 2 127.0.0.1 >nul\r\n";
        for index in 0..runs.len() {
            script += &format!("@findstr /b /c:\"end {index} \" \"{status_file}\" >nul || goto waiting\r\n");
        }
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
pub(super) fn write_private(path: &Path, text: &str, mode: u32) -> std::io::Result<()> {
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

/// How a batch of runs is run in a terminal: all at once, or in turn; and whether the terminal's left to close if they all succeed.
#[derive(Clone, Copy, Default)]
pub(super) struct How {
    pub(super) together: bool,
    pub(super) close: bool,
    /// Whether the terminal has to be told to close the window, as macOS's Terminal does, unless set otherwise.
    #[cfg_attr(windows, allow(dead_code))]
    pub(super) close_window: bool,
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
pub(super) enum Outcome {
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

/// One run in a batch, as it might be typed, and how it's going.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct Progress {
    pub(super) shown: String,
    pub(super) outcome: Outcome,
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
pub(super) struct JobState {
    /// The terminal it's running a batch of runs in, if it is: one batch at a time.
    terminal: Option<Terminal>,
    /// The runs of that batch, or the last, and the settings they were of: they're shown until those change.
    pub(super) runs: Vec<Progress>,
    pub(super) settings: Option<CommandSettings>,
    /// What went wrong the last time it tried to run.
    problem: Option<String>,
    /// What to send on, once it's done, if anything's connected after it.
    chain: Option<Chain>,
}

/// What's sent on once a batch has run (see `Then`): the files it ran on, to what's connected after it, and when.
pub(super) struct Chain {
    pub(super) handle: TriggerHandle<Trigger>,
    pub(super) then: Then,
    pub(super) files: Vec<PathBuf>,
}

impl Chain {
    /// Sends the files on, if the runs, as they ended, call for it: never if any was stopped (or didn't run).
    pub(super) fn send_after(self, runs: &[Progress]) {
        let stopped = runs.iter().any(|r| matches!(r.outcome, Outcome::Ended { code: None, .. } | Outcome::NotRun));
        let failed = runs.iter().any(|r| matches!(r.outcome, Outcome::Ended { code: Some(code), .. } if code != 0));
        let send = !stopped
            && match self.then {
                Then::OnSuccess => !failed,
                Then::OnFailure => failed,
                Then::Always => true,
            };
        if send {
            self.handle.trigger(RunOnFiles(self.files));
        }
    }
}

/// What's been run: shared with the thread following it (see `watch`).
#[derive(Clone, Default)]
pub(super) struct Job(pub(super) Arc<Mutex<JobState>>);

impl Job {
    /// Says what's wrong, in the node's window and the log: it may have been nobody's doing, after a recording.
    pub(super) fn fail(&self, problem: impl Into<String>) {
        let problem = problem.into();
        log::error!("Run Command: {problem}");
        self.0.lock().unwrap().problem = Some(problem);
    }

    pub(super) fn problem(&self) -> Option<String> {
        self.0.lock().unwrap().problem.clone()
    }

    pub(super) fn running(&self) -> bool {
        self.0.lock().unwrap().terminal.is_some()
    }

    /// The runs of the batch going, or of the last, if that was with `settings`.
    pub(super) fn runs_with(&self, settings: &CommandSettings) -> Option<Vec<Progress>> {
        let state = self.0.lock().unwrap();
        (state.terminal.is_some() || state.settings.as_ref() == Some(settings)).then(|| state.runs.clone())
    }

    /// Runs `runs`, made with `settings`, in turn in a new terminal, unless it's running already; then, as `chain`
    /// says, sends on what they ran on.
    pub(super) fn start(&self, settings: &CommandSettings, runs: &[Run], chain: Option<Chain>) {
        {
            let mut state = self.0.lock().unwrap();
            if state.terminal.is_some() {
                drop(state);
                return self.fail("It was still running, so it didn't run on what was sent");
            }
            state.problem = None;
            state.runs = runs.iter().map(|run| Progress { shown: run.shown(), outcome: Outcome::Waiting }).collect();
            state.settings = Some(settings.clone());
            state.chain = chain;
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
                (state.runs, state.settings, state.chain) = (Vec::new(), None, None);
                drop(state);
                self.fail(problem);
            }
        }
    }

    /// Follows the terminal, on a thread of its own, till it ends: it's timed here, so it needn't be shown.
    pub(super) fn watch(&self) {
        let job = self.clone();
        let watch = move || {
            loop {
                std::thread::sleep(Duration::from_millis(50));
                let mut state = job.0.lock().unwrap();
                let JobState { terminal, runs, chain, .. } = &mut *state;
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
                    // Sent on before it's done, so whatever sees it done sees what it sent.
                    if let Some(chain) = chain.take() {
                        chain.send_after(runs);
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
    pub(super) fn stop(&self) {
        if let Some(terminal) = self.0.lock().unwrap().terminal.as_mut() {
            terminal.process.stop();
        }
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
        assert!(lines.contains(&"call \"C:\\Program Files\\klusta.exe\" 100%%%%.set \"a b\""), "{script}");
        assert!(lines.contains(&"@>>\"C:\\status\" echo end 0 %errorlevel%"), "{script}");
        let pause = lines.iter().position(|line| *line == "@pause").unwrap();
        assert!(pause < lines.iter().position(|line| *line == ":run0").unwrap(), "runs are called, not fallen into");
    }

    #[test]
    fn a_batch_file_quotes_only_what_has_to_be() {
        let run = Run { program: "cmd".into(), arguments: vec!["/C".into(), "exit 3".into(), "a&b".into()] };
        let script = batch_script(Path::new("C:\\here"), Path::new("C:\\status"), &[run], false, false);
        assert!(script.contains("call cmd /C \"exit 3\" \"a&b\"\r\n"), "a quoted /C isn't one to cmd: {script}");
    }

    #[test]
    fn a_batch_file_starts_copies_of_itself_to_run_them_together() {
        let script = batch_script(Path::new("C:\\here"), Path::new("C:\\status"), &[run(), run()], true, false);
        assert!(script.contains("@start \"\" /b cmd /c call \"%~f0\" 1\r\n"), "{script}");
        assert!(script.contains("@findstr /b /c:\"end 1 \" \"C:\\status\" >nul || goto waiting"), "{script}");
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

    /// A batch file, as it ran: what it was, what it said, and what it left in its status file. All of it, should a
    /// test fail, as it can only be seen in CI.
    #[derive(Debug)]
    #[allow(dead_code, reason = "read in what a failing test says, through Debug")]
    struct Ran {
        script: String,
        timed_out: bool,
        stdout: String,
        stderr: String,
        status: String,
        still_there: bool,
    }

    /// Runs `runs` from a batch file, as a terminal would, but with nobody to press a key, for 30 seconds at most.
    fn run_batch(runs: &[Run], together: bool, close: bool) -> Ran {
        let (path, status, out, err) =
            (temporary_path("bat"), temporary_path("status"), temporary_path("out"), temporary_path("err"));
        let script = batch_script(&std::env::temp_dir(), &status, runs, together, close);
        std::fs::write(&path, &script).unwrap();
        let mut child = Command::new("cmd")
            .arg("/C")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&out).unwrap())
            .stderr(std::fs::File::create(&err).unwrap())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut timed_out = false;
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                timed_out = true;
                let pid = child.id().to_string();
                let _ = Command::new("taskkill").args(["/PID", &pid, "/T", "/F"]).output();
                let _ = child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let read = |path: &PathBuf| String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned();
        let ran = Ran {
            script,
            timed_out,
            stdout: read(&out),
            stderr: read(&err),
            status: read(&status),
            still_there: path.exists(),
        };
        for path in [&path, &status, &out, &err] {
            let _ = std::fs::remove_file(path);
        }
        ran
    }

    #[test]
    fn a_batch_file_says_what_programs_said_and_deletes_itself() {
        let ran = run_batch(&[cmd("echo said 100%"), cmd("exit 3")], false, false);
        assert!(!ran.timed_out, "{ran:#?}");
        assert!(ran.status.contains("end 0 0") && ran.status.contains("end 1 3"), "{ran:#?}");
        assert!(ran.stdout.contains("said 100%"), "{ran:#?}");
        assert!(ran.stdout.contains("Failed, exit code 3"), "{ran:#?}");
        assert!(ran.stdout.contains("Finished"), "{ran:#?}");
        assert!(!ran.still_there, "{ran:#?}");
    }

    #[test]
    fn a_batch_file_only_waits_if_something_failed() {
        for (command, waits) in [("exit 0", false), ("exit 2", true)] {
            let ran = run_batch(&[cmd(command)], false, true);
            assert!(!ran.timed_out, "{ran:#?}");
            assert_eq!(ran.stdout.contains("Finished."), waits, "{command}: {ran:#?}");
        }
    }

    #[test]
    fn a_batch_file_runs_them_together() {
        let ran = run_batch(&[cmd("ping -n 2 127.0.0.1 >nul"), cmd("exit 4")], true, false);
        assert!(!ran.timed_out, "{ran:#?}");
        assert!(ran.status.contains("end 0 0") && ran.status.contains("end 1 4"), "{ran:#?}");
    }

    #[test]
    fn a_terminal_can_be_stopped() {
        let job = Job::default();
        let ping = ["-n", "60", "127.0.0.1"].map(String::from).to_vec();
        job.start(&CommandSettings::default(), &[Run { program: "ping".into(), arguments: ping }], None);
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
