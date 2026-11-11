//! The program's log, for the dev tools' console: a logger (for the `log` crate's macros, which egui and wgpu
//! use too) keeping the latest lines in memory, and echoing them to stderr.
//!
//! Logging threads don't wait on each other, or on whoever reads the log: each message goes on a bounded lock-free
//! queue, and one consumer (the GUI thread, as it draws, see `drain`) takes them off to keep and echo. If the
//! queue is full (the GUI not drawing for a while, say), messages are dropped and counted rather than waited for.
//! Logging still formats the message into a `String`, so isn't for every pass: log what changes, not what happens.
//!
//! Each line says which thread logged it and, from within a pass, which pass (and node) it was: workers say
//! which pass they're running with `in_pass`.
//!
//! Everything this program logs is kept, at every level; other crates' only their warnings and errors. Messages
//! may style themselves with ANSI escape codes (colours, bold...), as for a terminal.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once, OnceLock};
use std::thread::Thread;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::engine::NodeId;

/// Lines kept, beyond which the oldest go.
const KEPT: usize = 10_000;
/// Messages that can wait to be taken off the queue.
const QUEUED: usize = 4096;

/// One line of the log. A message over several lines is kept as several, all but the first continuing it.
#[derive(Clone, Debug)]
pub struct Line {
    /// Seconds since the program started.
    pub time: f64,
    pub level: Level,
    /// Where it's from: a module path, e.g. "ephys_rs::engine::run".
    pub target: Cow<'static, str>,
    /// The thread that logged it.
    pub thread: Thread,
    /// The pass it was logged in, if it was.
    pub pass: Option<PassId>,
    pub text: String,
    pub continues: bool,
}

/// A pass, and the node it was running when it logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassId {
    /// The epoch the pass is of (see `engine::Runner`).
    pub epoch: u64,
    pub sequence: u64,
    pub node: Option<NodeId>,
    /// The node's type, e.g. "Spike Detector".
    pub node_type: Option<&'static str>,
}

thread_local! {
    /// The pass this thread is running, if it's a worker running one.
    static PASS: Cell<Option<PassId>> = const { Cell::new(None) };
}

/// Runs `f` as (part of) pass `pass`, so what it logs says so.
pub fn in_pass<R>(pass: PassId, f: impl FnOnce() -> R) -> R {
    /// Puts back the pass the thread was running before, however `f` ends.
    struct Restore(Option<PassId>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PASS.set(self.0);
        }
    }
    let _restore = Restore(PASS.replace(Some(pass)));
    f()
}

/// The pass this thread's running, if it's a worker running one (for a crash report).
pub fn current_pass() -> Option<PassId> {
    PASS.get()
}

/// A message as logged, waiting on the queue.
struct Message {
    time: f64,
    level: Level,
    target: Cow<'static, str>,
    thread: Thread,
    pass: Option<PassId>,
    text: String,
}

/// The lines kept for the console, and counts of what's been logged, kept or not.
#[derive(Default)]
struct Kept {
    lines: VecDeque<Line>,
    /// Lines ever kept, so a viewer can tell when there are new ones.
    count: u64,
    /// Errors and warnings logged since the console was last cleared (or the program started), including those no
    /// longer kept.
    errors: u64,
    warnings: u64,
}

impl Kept {
    /// Forgets the lines, and the errors and warnings counted: `count` carries on, so a viewer sees the change.
    fn clear(&mut self) {
        self.lines.clear();
        (self.errors, self.warnings) = (0, 0);
    }
}

/// The logger: logging threads queue their messages, and the console's frame takes them off the queue into what's
/// kept, so logging never waits on the console.
struct Logger {
    start: Instant,
    queue: Sender<Message>,
    incoming: Receiver<Message>,
    /// Only the consumer takes this, so it's never contended by logging threads.
    kept: Mutex<Kept>,
    dropped: AtomicU64,
    /// Whether to echo lines to stderr.
    echo: bool,
}

/// This program's modules, which log at every level.
const OURS: &str = env!("CARGO_CRATE_NAME");

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Warn || metadata.target().starts_with(OURS)
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // The target is almost always the module's path, which needn't be copied.
        let target = match record.module_path_static() {
            Some(path) if path == record.target() => Cow::Borrowed(path),
            _ => Cow::Owned(record.target().to_string()),
        };
        let message = Message {
            time: self.start.elapsed().as_secs_f64(),
            level: record.level(),
            target,
            thread: std::thread::current(),
            pass: PASS.get(),
            text: record.args().to_string(),
        };
        if self.queue.try_send(message).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn flush(&self) {}
}

impl Logger {
    /// Takes the messages waiting off the queue, to keep (and echo).
    fn drain(&self) {
        if self.incoming.is_empty() {
            return;
        }
        let mut kept = self.kept.lock().unwrap();
        for message in self.incoming.try_iter() {
            let thread = message.thread.name().unwrap_or("?");
            let line = format!("{:9.3} {:5} [{thread}] {}", message.time, message.level, message.text);
            crate::crash::note(&line);
            kept.errors += (message.level == Level::Error) as u64;
            kept.warnings += (message.level == Level::Warn) as u64;
            if self.echo && message.level <= Level::Info {
                eprintln!("{line}");
            }
            for (i, text) in message.text.lines().enumerate() {
                kept.lines.push_back(Line {
                    time: message.time,
                    level: message.level,
                    target: message.target.clone(),
                    thread: message.thread.clone(),
                    pass: message.pass,
                    text: text.to_string(),
                    continues: i > 0,
                });
                kept.count += 1;
            }
        }
        let excess = kept.lines.len().saturating_sub(KEPT);
        kept.lines.drain(..excess);
    }
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Installs the logger, if it isn't already (the tests share one). `echo` says whether to copy lines to stderr.
pub fn init(echo: bool) {
    static INSTALL: Once = Once::new();
    // Others calling at the same time wait until it's installed.
    INSTALL.call_once(|| {
        let logger = LOGGER.get_or_init(|| {
            let (queue, incoming) = crossbeam_channel::bounded(QUEUED);
            Logger { start: Instant::now(), queue, incoming, kept: Mutex::default(), dropped: AtomicU64::new(0), echo }
        });
        if log::set_logger(logger).is_ok() {
            log::set_max_level(LevelFilter::Trace);
        }
    });
}

/// Takes the messages logged since last time off the queue, to keep. The GUI calls this as it draws.
pub fn drain() {
    if let Some(logger) = LOGGER.get() {
        logger.drain();
    }
}

/// Runs `f` on the lines kept (taking any waiting first), oldest first, with how many lines have ever been kept.
pub fn with_lines<R>(f: impl FnOnce(&VecDeque<Line>, u64) -> R) -> R {
    match LOGGER.get() {
        Some(logger) => {
            logger.drain();
            let kept = logger.kept.lock().unwrap();
            f(&kept.lines, kept.count)
        }
        None => f(&VecDeque::new(), 0),
    }
}

/// How many errors and warnings have been logged since the console was last cleared, or the program started.
pub fn totals() -> (u64, u64) {
    let Some(logger) = LOGGER.get() else { return (0, 0) };
    logger.drain();
    let kept = logger.kept.lock().unwrap();
    (kept.errors, kept.warnings)
}

/// The last `count` lines logged, as text, without waiting for the log (for a crash report: none, if it's in use).
pub fn recent(count: usize) -> Vec<String> {
    let Some(logger) = LOGGER.get() else { return Vec::new() };
    let Ok(kept) = logger.kept.try_lock() else { return Vec::new() };
    let line = |time: f64, level: Level, thread: &Thread, text: &str| {
        format!("{time:9.3} {level:5} [{}] {text}", thread.name().unwrap_or("?"))
    };
    let mut lines: Vec<String> = kept.lines.iter().map(|l| line(l.time, l.level, &l.thread, &l.text)).collect();
    lines.extend(logger.incoming.try_iter().map(|m| line(m.time, m.level, &m.thread, &m.text)));
    let excess = lines.len().saturating_sub(count);
    lines.drain(..excess);
    lines
}

/// Messages dropped because the queue was full.
pub fn dropped() -> u64 {
    LOGGER.get().map_or(0, |l| l.dropped.load(Ordering::Relaxed))
}

/// Forgets the lines kept, and the errors and warnings counted, for the status bar to count afresh.
pub fn clear() {
    if let Some(logger) = LOGGER.get() {
        logger.drain();
        logger.kept.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a `Kept` of its own: clearing the log itself would clear other tests' lines.
    #[test]
    fn clearing_forgets_the_errors_and_warnings_too() {
        let mut kept = Kept { count: 5, errors: 2, warnings: 3, ..Default::default() };
        kept.clear();
        assert_eq!((kept.errors, kept.warnings, kept.count), (0, 0, 5));
    }

    #[test]
    fn errors_and_warnings_are_counted() {
        init(false);
        let (errors, warnings) = totals();
        log::error!("an error");
        log::warn!("a warning");
        log::info!("not either");
        let (more_errors, more_warnings) = totals();
        // Other tests log too.
        assert!(more_errors > errors && more_warnings > warnings);
    }

    #[test]
    fn keeps_our_lines_and_others_warnings() {
        init(false);
        log::debug!("first\nsecond");
        log::debug!(target: "wgpu_core::device", "not kept");
        log::warn!(target: "wgpu_core::device", "kept");
        // Other tests log at the same time, on other threads.
        let here = |l: &&Line| l.thread.id() == std::thread::current().id();
        let lines: Vec<(String, bool)> = with_lines(|lines, _| {
            let ours: Vec<&Line> = lines.iter().filter(here).collect();
            ours[ours.len() - 3..].iter().map(|l| (l.text.clone(), l.continues)).collect()
        });
        let expected = [("first", false), ("second", true), ("kept", false)].map(|(t, c)| (t.to_string(), c));
        assert_eq!(lines, expected);
    }

    #[test]
    fn says_which_thread_and_pass() {
        init(false);
        let pass = PassId { epoch: 2, sequence: 40, node: Some(NodeId(7)), node_type: Some("Spike Detector") };
        std::thread::Builder::new()
            .name("pass worker 9".into())
            .spawn(move || in_pass(pass, || log::info!("from a pass")))
            .unwrap()
            .join()
            .unwrap();
        let line = with_lines(|lines, _| lines.iter().rev().find(|l| l.text == "from a pass").cloned()).unwrap();
        assert_eq!(line.thread.name(), Some("pass worker 9"));
        assert_eq!(line.pass, Some(pass));
    }
}
