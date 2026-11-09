//! Acquiring from a board on a thread of its own, so samples are collected as they arrive whatever the passes
//! are doing. They're decoded straight into a pool of a pass's buffers (`Samples`), which the Acquisition Board
//! node's passes swap for the workers' own (see `crate::standard::pool`): each pass takes whatever's
//! arrived since the last, and nothing's copied. The digital inputs' changes are found as they're decoded. The
//! thread's status (what it's doing, the sample rate, losses) is shared for display. If the board can't be opened,
//! or fails, it keeps trying.
//!
//! A new epoch doesn't open the board again: if nothing it's acquiring has changed, its `Capture` carries on with the
//! one acquiring (new state's made before the old's let go of), and the same pool. A new configuration does, once the
//! old has closed. It's closed once nothing's using it, when the run stops.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{BoardInfo, Config, Samples, Source};
use crate::standard::ephys::{Continuous, Event, Events};
use crate::standard::pool::{Pool, SLACK_PASSES};

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
    /// Samples lost because the passes were so far behind there was no buffer to put them in.
    pub not_taken: u64,
    /// Samples skipped because the board's inputs aren't those expected.
    pub mismatched: u64,
    /// Digital input changes lost because a pass's buffer had no room for them.
    pub lost_events: u64,
    /// Samples per second, as they arrive, over the last couple of seconds.
    pub rate: f64,
}

/// The channels the passes take: each pass's buffers are made for them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub ephys: Continuous,
    pub aux: Continuous,
    pub adc: Continuous,
    pub events: Events,
}

impl Shape {
    fn samples(&self) -> Samples {
        Samples::new(self.ephys, self.aux, self.adc, self.events)
    }
}

struct Shared {
    stop: AtomicBool,
    status: Mutex<Status>,
}

/// A board acquiring on its own thread, until the last `Capture` using it lets go.
struct Running {
    shared: Arc<Shared>,
    pool: Pool<Samples>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A board acquiring, and what it's acquiring. Weak, so a board closes once nothing's using it.
struct Entry {
    source: Source,
    config: Config,
    shape: Shape,
    capture: Weak<Running>,
}

/// The boards acquiring.
static RUNNING: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// The boards open, or still closing.
static IN_USE: Mutex<Vec<Source>> = Mutex::new(Vec::new());

/// Having a board to open, until it's closed: a new configuration of a board has to wait for the old one to close.
struct Claim(Source);

impl Claim {
    fn take(source: &Source) -> Option<Self> {
        let mut in_use = IN_USE.lock().unwrap();
        if in_use.contains(source) {
            return None;
        }
        in_use.push(source.clone());
        Some(Claim(source.clone()))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        IN_USE.lock().unwrap().retain(|source| *source != self.0);
    }
}

/// An epoch's use of a board acquiring.
pub struct Capture {
    running: Arc<Running>,
}

impl Capture {
    /// Uses `source`, to acquire at `config`, into buffers for `shape`: carrying on with it, if it's doing that already,
    /// otherwise opening it.
    pub fn start(source: Source, config: Config, shape: Shape) -> Self {
        let mut running = RUNNING.lock().unwrap();
        running.retain(|entry| entry.capture.strong_count() > 0);
        let same = |entry: &&Entry| entry.source == source && entry.config == config && entry.shape == shape;
        if let Some(capture) = running.iter().find(same).and_then(|entry| entry.capture.upgrade()) {
            log::info!("Carrying on with the board");
            return Capture { running: capture };
        }
        let shared = Arc::new(Shared { stop: AtomicBool::new(false), status: Mutex::new(Status::default()) });
        let bytes = shape.samples().bytes();
        let pool = Pool::new((0..SLACK_PASSES).map(|_| shape.samples()), bytes);
        let (thread_shared, thread_pool) = (shared.clone(), pool.clone());
        let (thread_source, thread_config) = (source.clone(), config.clone());
        let thread = std::thread::Builder::new()
            .name("board capture".into())
            .spawn(move || capture(&thread_shared, &thread_source, &thread_config, shape, &thread_pool))
            .expect("capture thread");
        let capture = Arc::new(Running { shared, pool, thread: Some(thread) });
        running.push(Entry { source, config, shape, capture: Arc::downgrade(&capture) });
        Capture { running: capture }
    }

    /// The buffers the passes swap with.
    pub fn pool(&self) -> &Pool<Samples> {
        &self.running.pool
    }

    /// Copies the status into `status`, reusing its memory.
    pub fn status_into(&self, status: &mut Status) {
        status.clone_from(&self.running.shared.status.lock().unwrap());
    }
}

fn capture(shared: &Shared, source: &Source, config: &Config, shape: Shape, pool: &Pool<Samples>) {
    let stopping = || shared.stop.load(Ordering::Relaxed);
    let set_state = |state: State| {
        let mut status = shared.status.lock().unwrap();
        // Logged when it changes, not every time it's tried again.
        if let (State::Problem(problem), true) = (&state, status.state != state) {
            log::warn!("{problem}");
        }
        status.state = state;
    };
    let wait = || {
        let retry_at = Instant::now() + Duration::from_millis(500);
        while !stopping() && Instant::now() < retry_at {
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let lines = shape.events.lines.min(32);
    let mask = if lines == 32 { u32::MAX } else { (1 << lines) - 1 };
    while !stopping() {
        // Before `board`, so it's let go of after it's closed.
        let Some(_claim) = Claim::take(source) else {
            set_state(State::Problem("waiting for board: it's in use, or still closing".into()));
            wait();
            continue;
        };
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
            log::info!("Acquiring from {}", board.info().description);
            for warning in &status.warnings {
                log::warn!("{warning}");
            }
        }
        // Samples that can't go to the passes (not the channels they expect, or no buffer for them) are decoded
        // here, and dropped.
        let (ephys, aux, adc) = board.layout();
        let channels = |n: usize, like: Continuous| Continuous { channels: n as u32, ..like };
        let mut aside = Samples::new(
            channels(ephys, shape.ephys),
            channels(aux, shape.aux),
            channels(adc, shape.adc),
            shape.events,
        );
        let mismatched = (ephys, aux, adc) != expected_layout(&shape);
        if mismatched {
            log::warn!(
                "The board's sending {ephys} headstage channels, not the {} set: skipping them",
                shape.ephys.channels
            );
        }
        let (mut next, mut ttl, mut last_ttl) = (None::<u64>, Vec::new(), 0u32);
        let mut recent = VecDeque::new(); // (when, samples) within RATE_WINDOW
        while !stopping() {
            match board.fetch() {
                Ok(true) => {}
                Ok(false) => continue, // nothing came yet: check for stopping, and try again
                Err(e) => {
                    set_state(State::Problem(format!("board error, reconnecting: {e}")));
                    break;
                }
            }
            ttl.clear();
            let mut lost_events = 0;
            let into_pool = (!mismatched)
                .then(|| {
                    pool.fill(Samples::clear, |samples| {
                        let decoded = board.decode(samples, &mut ttl)?;
                        let at = samples.len() - decoded.samples;
                        for (i, &word) in ttl.iter().enumerate() {
                            let changed = (word ^ last_ttl) & mask;
                            for line in (0..lines).filter(|line| changed & (1 << line) != 0) {
                                let events = &mut samples.events.events;
                                if events.len() < events.capacity() {
                                    let high = word & (1 << line) != 0;
                                    events.push(Event { sample: (at + i) as u32, line: line as u8, high, time: None });
                                } else {
                                    lost_events += 1;
                                }
                            }
                            last_ttl = word;
                        }
                        Ok::<_, std::io::Error>((decoded, samples.room() == 0))
                    })
                })
                .flatten();
            let (decoded, aside_why) = match into_pool {
                Some(Ok((decoded, full))) => {
                    if full {
                        pool.filled();
                    }
                    (Ok(decoded), None)
                }
                Some(Err(e)) => (Err(e), None),
                None => {
                    // Not the channels expected, or the passes so far behind the pool's empty.
                    if !mismatched {
                        pool.overflowed();
                    }
                    aside.clear();
                    let decoded = board.decode(&mut aside, &mut ttl);
                    if let Some(&word) = ttl.last() {
                        last_ttl = word;
                    }
                    (decoded, Some(mismatched))
                }
            };
            let decoded = match decoded {
                Ok(decoded) => decoded,
                Err(e) => {
                    set_state(State::Problem(format!("board error, reconnecting: {e}")));
                    break;
                }
            };
            let now = Instant::now();
            let mut status = shared.status.lock().unwrap();
            let samples = decoded.samples as u64;
            status.samples += samples;
            status.lost_events += lost_events;
            match aside_why {
                Some(true) => status.mismatched += samples,
                Some(false) => {
                    if status.not_taken == 0 {
                        log::error!("The passes aren't keeping up with the board: samples are being lost");
                    }
                    status.not_taken += samples;
                }
                None => {}
            }
            if let Some(next) = next {
                status.board_dropped += decoded.first.saturating_sub(next);
            }
            next = Some(decoded.first + samples);
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

/// The channels of each kind the passes expect.
fn expected_layout(shape: &Shape) -> (usize, usize, usize) {
    (shape.ephys.channels as usize, shape.aux.channels as usize, shape.adc.channels as usize)
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
    use crate::standard::ephys::board::{Bandwidth, Headstage};

    /// Takes from `capture`'s pool as passes do, for `seconds` or until `wanted` samples: how many, the layout,
    /// digital changes, and the biggest headstage value.
    fn take_like_passes(
        capture: &Capture,
        wanted: usize,
        seconds: u64,
    ) -> (usize, Option<(usize, usize, usize)>, usize, f32) {
        let (mut samples, mut layout, mut changes, mut peak) = (0, None, 0, 0f32);
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while samples < wanted && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            // All that's waiting, as passes catching up would (a sleep can take longer than a buffer holds, on a busy
            // machine).
            while let Some(taken) = capture.pool().take(|s| s.is_empty()) {
                samples += taken.len();
                layout = Some(taken.layout());
                changes += taken.events.events.len();
                for c in 0..taken.ephys.channels {
                    peak = taken.ephys.channel(c).iter().fold(peak, |m, v| m.max(v.abs()));
                }
                assert_eq!(taken.events.pass_samples, taken.len());
                capture.pool().give_back(taken);
            }
        }
        (samples, layout, changes, peak)
    }

    fn shape(ephys: u32, aux: u32, rate: f32) -> Shape {
        let continuous = |channels| Continuous { channels, sample_rate: rate };
        Shape {
            ephys: continuous(ephys),
            aux: continuous(aux),
            adc: continuous(8),
            events: Events { lines: 8, sample_rate: rate },
        }
    }

    #[test]
    fn epochs_carry_on_with_the_board_and_a_new_configuration_waits_for_it_to_close() {
        // The test board takes one connection, so samples coming later show it wasn't opened again.
        let source = Source::Fake { address: crate::standard::ephys::board::serve_test_board() };
        let config = Config { sample_rate: 30000, bandwidth: Bandwidth::default(), headstages: Vec::new() };
        let first = Capture::start(source.clone(), config.clone(), shape(64, 6, 30000.0));
        assert!(take_like_passes(&first, 3000, 10).0 >= 3000);
        let second = Capture::start(source.clone(), config, shape(64, 6, 30000.0));
        assert!(Arc::ptr_eq(&first.running, &second.running), "the same board");
        drop(first);
        assert!(take_like_passes(&second, 3000, 10).0 >= 3000, "carrying on");

        assert!(Claim::take(&source).is_none(), "it's open");
        let board = Arc::downgrade(&second.running);
        drop(second);
        assert!(board.upgrade().is_none());
        assert!(Claim::take(&source).is_some(), "closed");
    }

    #[test]
    fn passes_take_the_samples_as_they_come_in_pass_sized_buffers() {
        let source = Source::Fake { address: crate::standard::ephys::board::serve_test_board() };
        let config = Config { sample_rate: 30000, bandwidth: Bandwidth::default(), headstages: Vec::new() };
        let capture = Capture::start(source, config, shape(64, 6, 30000.0));
        let (samples, layout, changes, peak) = take_like_passes(&capture, 30000, 10);
        let mut status = Status::default();
        capture.status_into(&mut status);
        assert!(samples >= 30000, "a second; status {status:?}");
        assert_eq!(layout, Some((64, 6, 8)));
        assert!(changes >= 2, "the sync pulse goes up and down every half second");
        assert!((20.0..5000.0).contains(&peak), "microvolts: {peak}");
        assert_eq!((status.board_dropped, status.not_taken, status.mismatched), (0, 0, 0));
        assert!(capture.pool().status().swaps > 0);
    }

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
        let capture = Capture::start(source, config, shape(48, 6, 20000.0));
        let (samples, layout, changes, peak) = take_like_passes(&capture, 20000, 15);
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
        assert!(changes >= 2, "the sync pulse goes up and down each second");
        assert_eq!(status.board_dropped, 0);
    }
}
