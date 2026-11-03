//! Writers: nodes that record what's connected to them while the graph is recording, to LTX files (`ltx`) or Open
//! Ephys's binary format (`raw`).
//!
//! A writer's passes only hand their inputs out (see `Node::copies`), in chunks to a thread of its own (`Writing`),
//! which writes them to disk. So passes never wait on the disk. The chunks are a pool (see
//! `crate::standard_components::pool`), made before recording and handed back once written, so nothing's allocated;
//! a writer whose inputs are its own to take (see `Node::takes`) swaps them into a chunk rather than copying.
//!
//! Before recording starts, a writer opens its files and starts its thread (`Node::prepare_to_record`), so passes
//! only record once it's ready. (If it starts recording without having prepared, after the graph's changed
//! while recording, say, its thread opens the files as it starts.) Stopping lets the thread write what's
//! queued and finish the files in the background (see `crate::core::background`).

pub mod ltx;
mod npy;
pub mod raw;

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use eframe::egui::{self, RichText};

use crate::core::engine::PoolStatus;
use crate::core::theme::Colours;
use crate::standard_components::pool::{Pool, SLACK_PASSES};

/// Why a writer writes on a thread of its own, for the dev tools.
pub const WHY_A_THREAD: &str = "On a thread of its own so the disk can stall (flushing its cache, a virus scanner, a \
                                slow drive) without holding up the passes: the files are written in order, so each \
                                pass would wait for the one before to finish writing.";

/// About the most memory a writer's pool of chunks takes (it's at least a few passes' chunks, though).
const POOL_BYTES: usize = 32 << 20;

/// How a writer's thread is getting on.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// Where it's writing (a file's or a folder's path, without the extension for LTX), while recording.
    pub path: Option<String>,
    pub bytes: u64,
    /// Where its chunks are, and how many were lost for want of one (the disk not keeping up).
    pub pool: PoolStatus,
    /// What went wrong, if anything did: it stops writing.
    pub problem: Option<String>,
}

/// Files a writer's thread writes to, for chunks of type `C`.
pub trait Files<C>: Send {
    /// Writes a chunk, returning the bytes written.
    fn write(&mut self, chunk: &C) -> io::Result<u64>;
    /// Fills in what's only known at the end (counts, durations), and closes the files.
    fn finish(self: Box<Self>) -> io::Result<()>;
}

/// A writer's thread, writing the chunks its passes send, while recording. Dropping it finishes the files.
pub struct Writing<C> {
    /// The chunks, unless the files couldn't be made.
    pool: Option<Pool<C>>,
    done: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    thread: Option<JoinHandle<()>>,
}

impl<C: Send + 'static> Writing<C> {
    /// Starts a thread called `name` writing to `files`, opened already (or failing to be, which it reports), with
    /// chunks made by `chunk`, each taking `bytes`.
    pub fn new(
        name: &str,
        path: String,
        files: io::Result<Box<dyn Files<C>>>,
        chunk: impl Fn() -> C,
        bytes: usize,
    ) -> Self {
        match files {
            Ok(files) => Self::start(name, path, move || Ok(files), chunk, bytes),
            Err(e) => {
                log::error!("Recording to {path}: couldn't create the files: {e}");
                let problem = Some(format!("couldn't create the files: {e}"));
                let status = Status { path: Some(path), problem, ..Default::default() };
                let done = Arc::new(AtomicBool::new(true));
                Writing { pool: None, done, status: Arc::new(Mutex::new(status)), thread: None }
            }
        }
    }

    /// Starts a thread called `name` that opens files with `open`, then writes the chunks sent to it: chunks made
    /// by `chunk`, each taking `bytes`.
    pub fn start(
        name: &str,
        path: String,
        open: impl FnOnce() -> io::Result<Box<dyn Files<C>>> + Send + 'static,
        chunk: impl Fn() -> C,
        bytes: usize,
    ) -> Self {
        let size = (POOL_BYTES / bytes.max(1)).clamp(4, SLACK_PASSES);
        let pool = Pool::new((0..size).map(|_| chunk()), bytes);
        let status = Arc::new(Mutex::new(Status { path: Some(path.clone()), ..Default::default() }));
        let done = Arc::new(AtomicBool::new(false));
        let (thread_status, thread_pool, thread_done) = (status.clone(), pool.clone(), done.clone());
        let fail = move |status: &Mutex<Status>, what: &str, e: io::Error| {
            log::error!("Recording to {path}: {what}: {e}");
            status.lock().unwrap().problem = Some(format!("{what}: {e}"));
        };
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let mut files = match open() {
                    Ok(files) => files,
                    Err(e) => return fail(&thread_status, "couldn't create the files", e),
                };
                // Until recording stops, then what's left.
                loop {
                    let chunk = match thread_pool.next_full(Duration::from_millis(20)) {
                        Some(chunk) => chunk,
                        None if thread_done.load(Ordering::Acquire) => match thread_pool.try_full() {
                            Some(chunk) => chunk,
                            None => break,
                        },
                        None => continue,
                    };
                    match files.write(&chunk) {
                        Ok(bytes) => thread_status.lock().unwrap().bytes += bytes,
                        Err(e) => return fail(&thread_status, "couldn't write", e),
                    }
                    thread_pool.give_back(chunk);
                }
                if let Err(e) = files.finish() {
                    fail(&thread_status, "couldn't finish the files", e);
                }
            })
            .expect("writer thread");
        Writing { pool: Some(pool), done, status, thread: Some(thread) }
    }

    /// An empty chunk to put a pass's data in, unless the disk's so far behind there are none (and the pass's data
    /// is lost, and counted).
    pub fn chunk(&self) -> Option<C> {
        let chunk = self.pool.as_ref()?.empty();
        if chunk.is_none() {
            let mut status = self.status.lock().unwrap();
            if status.problem.is_none() {
                log::error!("Recording can't keep up: data is being lost");
                status.problem = Some("the disk isn't keeping up: data is being lost".into());
            }
        }
        chunk
    }

    /// Hands a chunk to the thread to write.
    pub fn send(&self, chunk: C) {
        if let Some(pool) = &self.pool {
            pool.send(chunk);
        }
    }

    /// Copies the status into `status`.
    pub fn status_into(&self, status: &mut Status) {
        status.clone_from(&self.status.lock().unwrap());
        status.pool = self.pool.as_ref().map(Pool::status).unwrap_or_default();
    }
}

impl<C> Drop for Writing<C> {
    /// Lets the thread write what's queued and finish the files, in the background.
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            crate::core::background::finish_later(thread);
        }
    }
}

/// `directory` (a leading `~` meaning the home directory), with `name` in it.
pub fn path_in(directory: &str, name: &str) -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let directory = match (directory.strip_prefix("~"), home) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(directory),
    };
    directory.join(name)
}

/// A recording's name: the prefix, then when it started, e.g. "session_2026-11-01_143005".
pub fn session_name(prefix: &str, start: &chrono::DateTime<chrono::Local>) -> String {
    format!("{prefix}_{}", start.format("%Y-%m-%d_%H%M%S"))
}

/// What a writer's thread is up to, for its window and the dev tools.
pub fn status_ui(ui: &mut egui::Ui, status: Option<&Status>, theme: &Colours) {
    let small = |text: String| RichText::new(text).size(11.0);
    let Some(status) = status else {
        ui.label(small("Writes while recording.".into()).weak());
        return;
    };
    match (&status.path, &status.problem) {
        (_, Some(problem)) => {
            ui.label(small(problem.clone()).color(theme.error));
        }
        (Some(path), None) => {
            ui.label(small(format!("Recording to {path}")));
            ui.label(small(format!("{:.1} MB written", status.bytes as f64 / 1e6)).weak());
        }
        (None, None) => {
            ui.label(small("Not recording.".into()).weak());
        }
    }
    if status.pool.overflows > 0 {
        let lost = format!("{} passes' data lost: the disk isn't keeping up", status.pool.overflows);
        ui.label(small(lost).color(theme.error));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::Nodes;
    use crate::core::engine::{Graph, NodeId, NodeSet, Port, Recording, Role, Runner, Snapshot};
    use crate::standard_components::ephys::board::{Source, serve_test_board};

    fn add(graph: &mut Graph<Nodes>, name: &str) -> NodeId {
        let index = Nodes::TYPES.iter().position(|t| t.name == name).unwrap();
        graph.add(Nodes::create(index), [0.0, 0.0])
    }

    fn link(graph: &mut Graph<Nodes>, from: NodeId, output: usize, to: NodeId, input: usize) {
        let port = |node, role, index| Port { node, role, index };
        graph.connect(port(from, Role::Output, output), port(to, Role::Input, input)).unwrap();
    }

    #[test]
    fn records_while_recording_and_finishes_the_files() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-recording-{}", std::process::id()));
        let directory = dir.display().to_string();
        let mut graph = Graph::default();
        let board = add(&mut graph, "Acquisition Board");
        if let Some(Nodes::AcquisitionBoard(b)) = graph.get_mut(board).map(|e| &mut e.node) {
            b.source = Source::Fake { address: serve_test_board() };
        }
        let detector = add(&mut graph, "Spike Detector");
        let ltx = add(&mut graph, "Write LTX");
        let raw = add(&mut graph, "Write Raw");
        link(&mut graph, board, 0, detector, 0);
        link(&mut graph, detector, 0, ltx, 0);
        link(&mut graph, board, 0, ltx, 1);
        link(&mut graph, board, 0, raw, 0);
        link(&mut graph, board, 1, raw, 1);
        match graph.get_mut(ltx).map(|e| &mut e.node) {
            Some(Nodes::WriteLtx(w)) => w.directory = directory.clone(),
            _ => unreachable!(),
        }
        match graph.get_mut(raw).map(|e| &mut e.node) {
            Some(Nodes::WriteRaw(w)) => w.directory = directory.clone(),
            _ => unreachable!(),
        }

        let snapshot = Snapshot::new(0, graph, None);
        let runner = Runner::start(&snapshot, 2);
        // The writers open their files before recording starts.
        let recording = snapshot.with_recording(1, Some(Arc::new(Recording::prepare(&snapshot))));
        assert!(runner.update(&recording), "recording carries on the same run");
        let deadline = Instant::now() + Duration::from_secs(10);
        while runner.clock() < 1.0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        runner.update(&recording.with_recording(2, None));
        drop(runner);
        crate::core::background::wait();

        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
        let named = |suffix: &str| files.iter().find(|p| p.to_string_lossy().ends_with(suffix)).cloned();
        let egf = std::fs::read(named(".egf").expect("an .egf file")).unwrap();
        let text = String::from_utf8_lossy(&egf);
        let samples: u64 =
            text.split("num_EEG_samples ").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
        assert!(samples > 1000, "a second or so at 5 kHz, got {samples}");
        assert!(text.contains("data_end"), "finished");
        assert!(named(".1").is_some(), "the first tetrode's spikes");
        let folder = files.iter().find(|p| p.is_dir()).expect("the raw recording's folder");
        let dat =
            folder.join("Record Node 101/experiment1/recording1/continuous/ephys-rs-100.continuous/continuous.dat");
        let bytes = std::fs::metadata(dat).unwrap().len();
        assert!(bytes as f64 > 64.0 * 2.0 * 30000.0 * 0.5, "half a second or more of 64 channels, got {bytes} bytes");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
