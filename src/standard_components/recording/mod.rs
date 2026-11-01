//! Writers: nodes that record what's connected to them while the graph is recording, to LTX files (`ltx`) or Open
//! Ephys's binary format (`raw`).
//!
//! A writer's passes only copy their inputs out (see `Node::copies`), into chunks it hands to a thread of its own
//! (`Writing`), which writes them to disk. So passes never wait on the disk. Chunks are handed back to be reused,
//! so once there are enough of them, nothing's allocated.
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
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use eframe::egui::{self, RichText};

use crate::core::theme::Colours;
use crate::standard_components::widgets::queue_ui;

/// Chunks that can wait to be written: a few seconds' worth of passes.
pub const QUEUE_LEN: usize = 256;

/// How a writer's thread is getting on.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// Where it's writing (a file's or a folder's path, without the extension for LTX), while recording.
    pub path: Option<String>,
    pub bytes: u64,
    /// Chunks waiting to be written.
    pub queued: usize,
    /// Chunks lost because the queue was full (the disk not keeping up).
    pub dropped: u64,
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

/// A writer's thread, writing chunks copied from its passes, while recording. Dropping it finishes the files.
pub struct Writing<C> {
    chunks: Option<Sender<C>>,
    spares: Receiver<C>,
    status: Arc<Mutex<Status>>,
    thread: Option<JoinHandle<()>>,
}

impl<C: Default + Send + 'static> Writing<C> {
    /// Starts a thread called `name` writing to `files`, opened already (or failing to be, which it reports).
    pub fn new(name: &str, path: String, files: io::Result<Box<dyn Files<C>>>) -> Self {
        match files {
            Ok(files) => Self::start(name, path, move || Ok(files)),
            Err(e) => {
                log::error!("Recording to {path}: couldn't create the files: {e}");
                let problem = Some(format!("couldn't create the files: {e}"));
                let status = Status { path: Some(path), problem, ..Default::default() };
                let (_, spares) = crossbeam_channel::bounded(0);
                Writing { chunks: None, spares, status: Arc::new(Mutex::new(status)), thread: None }
            }
        }
    }

    /// Starts a thread called `name` that opens files with `open`, then writes the chunks sent to it.
    pub fn start(
        name: &str,
        path: String,
        open: impl FnOnce() -> io::Result<Box<dyn Files<C>>> + Send + 'static,
    ) -> Self {
        let (chunks, incoming) = crossbeam_channel::bounded::<C>(QUEUE_LEN);
        let (spare, spares) = crossbeam_channel::bounded(QUEUE_LEN);
        let status = Arc::new(Mutex::new(Status { path: Some(path.clone()), ..Default::default() }));
        let thread_status = status.clone();
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
                for chunk in incoming.iter() {
                    match files.write(&chunk) {
                        Ok(bytes) => thread_status.lock().unwrap().bytes += bytes,
                        Err(e) => return fail(&thread_status, "couldn't write", e),
                    }
                    let _ = spare.try_send(chunk);
                }
                if let Err(e) = files.finish() {
                    fail(&thread_status, "couldn't finish the files", e);
                }
            })
            .expect("writer thread");
        Writing { chunks: Some(chunks), spares, status, thread: Some(thread) }
    }

    /// An empty chunk to copy into: one written already, to reuse, if there is one.
    pub fn chunk(&self) -> C {
        self.spares.try_recv().unwrap_or_default()
    }

    /// Hands a chunk to the thread to write; if it's fallen too far behind, the chunk's lost (and counted).
    pub fn send(&self, chunk: C) {
        let Some(chunks) = &self.chunks else { return };
        if let Err(TrySendError::Full(_)) = chunks.try_send(chunk) {
            let mut status = self.status.lock().unwrap();
            if status.dropped == 0 {
                log::error!("Recording can't keep up: data is being lost");
            }
            status.dropped += 1;
        }
    }

    /// Copies the status into `status`.
    pub fn status_into(&self, status: &mut Status) {
        status.clone_from(&self.status.lock().unwrap());
        status.queued = self.chunks.as_ref().map_or(0, Sender::len);
    }
}

impl<C> Drop for Writing<C> {
    /// Lets the thread write what's queued and finish the files, in the background.
    fn drop(&mut self) {
        self.chunks = None;
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
    if status.path.is_some() {
        queue_ui(ui, theme, "chunks", status.queued, QUEUE_LEN);
    }
    if status.dropped > 0 {
        ui.label(small(format!("{} chunks lost: the disk isn't keeping up", status.dropped)).color(theme.error));
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
