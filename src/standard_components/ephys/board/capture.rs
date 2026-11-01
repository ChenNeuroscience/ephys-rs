//! Acquiring from a board on a thread of its own, so samples are collected as they arrive whatever the passes
//! are doing. Blocks of samples queue for the Acquisition Board node's next passes (which hand them back to be
//! reused), and the thread's status (what it's doing, the sample rate, losses) is shared for display. If the
//! board can't be opened, or fails, it keeps trying.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TrySendError};

use super::{Block, BoardInfo, Config, Source};

/// Blocks that can wait for the node's passes: a few seconds' worth, for most boards.
const QUEUE_LEN: usize = 512;
/// Seconds the measured sample rate is over.
const RATE_WINDOW: f64 = 2.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    /// Opening the board.
    #[default]
    Starting,
    Running,
    /// Something's wrong (e.g. no board); it keeps trying.
    Problem(String),
}

/// How acquisition is going.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub state: State,
    /// The open board, and what it has.
    pub info: Option<BoardInfo>,
    /// Problems setting the board up that don't stop it working.
    pub warnings: Vec<String>,
    pub samples: u64,
    /// Samples the board skipped (from gaps in its sample numbers).
    pub board_dropped: u64,
    /// Samples discarded because they weren't taken from the queue in time.
    pub queue_dropped: u64,
    /// Samples per second, as they arrive, over the last couple of seconds.
    pub rate: f64,
}

impl Status {
    /// Copies `other`, reusing this one's memory (for its text).
    pub fn copy_from(&mut self, other: &Status) {
        self.state.clone_from(&other.state);
        self.info.clone_from(&other.info);
        self.warnings.clone_from(&other.warnings);
        (self.samples, self.board_dropped, self.queue_dropped, self.rate) =
            (other.samples, other.board_dropped, other.queue_dropped, other.rate);
    }
}

struct Shared {
    stop: AtomicBool,
    status: Mutex<Status>,
}

/// A board acquiring on its own thread, until dropped.
pub struct Capture {
    shared: Arc<Shared>,
    blocks: Receiver<Block>,
    /// Blocks handed back, to reuse.
    spare: Sender<Block>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Starts opening `source`, to acquire at `config`.
    pub fn start(source: Source, config: Config) -> Self {
        let shared = Arc::new(Shared { stop: AtomicBool::new(false), status: Mutex::new(Status::default()) });
        let (sender, blocks) = crossbeam_channel::bounded(QUEUE_LEN);
        let (spare, spares) = crossbeam_channel::bounded(QUEUE_LEN);
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("board capture".into())
            .spawn(move || capture(&thread_shared, &source, &config, &sender, &spares))
            .expect("capture thread");
        Capture { shared, blocks, spare, thread: Some(thread) }
    }

    /// The next block waiting, if there is one.
    pub fn next(&self) -> Option<Block> {
        self.blocks.try_recv().ok()
    }

    /// Hands a block back, to be reused.
    pub fn recycle(&self, block: Block) {
        let _ = self.spare.try_send(block);
    }

    /// Copies the status into `status`, reusing its memory.
    pub fn status_into(&self, status: &mut Status) {
        status.copy_from(&self.shared.status.lock().unwrap());
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn capture(shared: &Shared, source: &Source, config: &Config, sender: &Sender<Block>, spares: &Receiver<Block>) {
    let stopping = || shared.stop.load(Ordering::Relaxed);
    let set_state = |state| shared.status.lock().unwrap().state = state;
    let wait = || {
        let retry_at = Instant::now() + Duration::from_millis(500);
        while !stopping() && Instant::now() < retry_at {
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    while !stopping() {
        let mut board = match super::open(source) {
            Ok(board) => board,
            Err(e) => {
                set_state(State::Problem(format!("waiting for board: {e}")));
                wait();
                continue;
            }
        };
        shared.status.lock().unwrap().info = Some(board.info().clone());
        if let Err(e) = board.start(config) {
            set_state(State::Problem(format!("couldn't start the board: {e}")));
            wait();
            continue;
        }
        {
            let mut status = shared.status.lock().unwrap();
            status.warnings = board.warnings();
            status.state = State::Running;
        }
        let mut next = None::<u64>;
        let mut recent = VecDeque::new(); // (when, samples) within RATE_WINDOW
        while !stopping() {
            let mut block = spares.try_recv().unwrap_or_default();
            match board.read(&mut block) {
                Ok(true) => {}
                Ok(false) => continue, // nothing came yet: check for stopping, and try again
                Err(e) => {
                    set_state(State::Problem(format!("board error, reconnecting: {e}")));
                    break;
                }
            }
            let (first, samples) = (block.first, block.samples as u64);
            let queue_full = match sender.try_send(block) {
                Ok(()) => false,
                Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Disconnected(_)) => return,
            };
            let now = Instant::now();
            let mut status = shared.status.lock().unwrap();
            status.samples += samples;
            status.queue_dropped += if queue_full { samples } else { 0 };
            if let Some(next) = next {
                status.board_dropped += first.saturating_sub(next);
            }
            next = Some(first + samples);
            recent.push_back((now, samples));
            while recent.front().is_some_and(|&(t, _)| now.duration_since(t).as_secs_f64() > RATE_WINDOW) {
                recent.pop_front();
            }
            let span = now.duration_since(recent.front().map_or(now, |&(t, _)| t)).as_secs_f64();
            if span > 0.5 {
                let counted: u64 = recent.iter().skip(1).map(|&(_, n)| n).sum();
                status.rate = counted as f64 / span;
            }
        }
    }
}

/// Finds out what `source` has, on a thread of its own: a board's headstages, say, before running. The
/// answer comes in the returned slot.
pub fn detect(source: Source) -> Arc<Mutex<Option<Result<BoardInfo, String>>>> {
    let slot = Arc::new(Mutex::new(None));
    let answer = slot.clone();
    std::thread::spawn(move || {
        let info = super::open(&source).map(|board| board.info().clone()).map_err(|e| e.to_string());
        *answer.lock().unwrap() = Some(info);
    });
    slot
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;
    use crate::standard_components::ephys::board::{Bandwidth, Headstage};

    /// End to end with synthetic_streams/fake_acquisition_board.py, so needs python3: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn acquires_from_the_fake_board() {
        let port = 27399;
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/synthetic_streams/fake_acquisition_board.py");
        let mut child = Command::new("python3")
            .args([script, "--port", &port.to_string(), "--headstages", "A1:32,C1:16"])
            .stdout(Stdio::null())
            .spawn()
            .expect("run python3");
        let source = Source::Fake { address: format!("127.0.0.1:{port}") };
        let config = Config { sample_rate: 20000, bandwidth: Bandwidth::default(), headstages: Vec::new() };
        let capture = Capture::start(source, config);
        let (mut samples, mut layout, mut ttl_changes, mut last_ttl, mut peak) = (0, None, 0, 0, 0f32);
        let deadline = Instant::now() + Duration::from_secs(15);
        while samples < 20000 && Instant::now() < deadline {
            let Some(block) = capture.next() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            samples += block.samples;
            layout = Some((block.channels(&block.ephys), block.channels(&block.aux), block.channels(&block.adc)));
            peak = block.ephys.iter().fold(peak, |m, v| m.max(v.abs()));
            for &word in &block.ttl {
                ttl_changes += (word != last_ttl) as u32;
                last_ttl = word;
            }
            capture.recycle(block);
        }
        let mut status = Status::default();
        capture.status_into(&mut status);
        drop(capture);
        child.kill().ok();
        child.wait().ok();
        assert!(samples >= 20000, "a second at 20 kHz; status {status:?}");
        let info = status.info.expect("the board described itself");
        let expected = [Headstage { port: "A1".into(), channels: 32 }, Headstage { port: "C1".into(), channels: 16 }];
        assert_eq!(info.headstages, expected);
        assert_eq!(layout, Some((48, 6, 8)), "headstage, aux and adc channels");
        assert!((50.0..2000.0).contains(&peak), "microvolts, as headstages see them: {peak}");
        assert!(ttl_changes >= 2, "the sync pulse goes up and down each second");
        assert_eq!(status.board_dropped, 0);
    }
}
