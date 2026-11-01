//! Capturing from a camera on a thread of its own, so frames are collected as they arrive whatever the
//! passes are doing. Frames queue for the Camera node's next pass, and the thread's status (what it's doing,
//! the frame rate, drops) is shared for display. If the camera can't be opened, or fails, it keeps trying.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TrySendError};

use super::{self as camera, CameraControls, CameraSettings, Frame, Source};

/// Frames that can wait for the next pass; enough for a second or more at most frame rates.
const QUEUE_LEN: usize = 64;
/// Seconds of frames the measured frame rate is over.
const RATE_WINDOW: f64 = 2.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    /// Opening the camera.
    #[default]
    Starting,
    Running,
    /// Something's wrong (e.g. no camera); it keeps trying.
    Problem(String),
}

/// How capture is going.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub state: State,
    /// The open camera, e.g. its model and serial number.
    pub description: String,
    /// Problems configuring the camera that don't stop it working.
    pub warnings: Vec<String>,
    /// What the open camera supports.
    pub controls: CameraControls,
    pub frames: u64,
    /// Inferred from gaps in the camera's frame ids.
    pub camera_dropped: u64,
    /// Frames discarded because they weren't taken from the queue in time.
    pub queue_dropped: u64,
    /// Frames per second, from the camera's timestamps over the last couple of seconds.
    pub fps: f64,
}

impl Status {
    /// Copies `other`, reusing this one's memory (for its text).
    pub fn copy_from(&mut self, other: &Status) {
        self.state.clone_from(&other.state);
        self.description.clone_from(&other.description);
        self.warnings.clone_from(&other.warnings);
        (self.controls, self.frames, self.camera_dropped, self.queue_dropped, self.fps) =
            (other.controls, other.frames, other.camera_dropped, other.queue_dropped, other.fps);
    }
}

struct Shared {
    stop: AtomicBool,
    /// What to apply to the camera; exposure and gain take effect as they change.
    settings: Mutex<CameraSettings>,
    status: Mutex<Status>,
}

/// A camera capturing on its own thread, until dropped.
pub struct Capture {
    shared: Arc<Shared>,
    frames: Receiver<Frame>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Starts opening `source` at `settings` (its frame rate fixed from now on).
    pub fn start(source: Source, settings: CameraSettings) -> Self {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            settings: Mutex::new(settings),
            status: Mutex::new(Status::default()),
        });
        let (sender, frames) = crossbeam_channel::bounded(QUEUE_LEN);
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("camera capture".into())
            .spawn(move || capture(&thread_shared, &source, &sender))
            .expect("capture thread");
        Capture { shared, frames, thread: Some(thread) }
    }

    /// Changes the exposure and gain (the frame rate only takes effect on restarting).
    pub fn apply(&self, settings: &CameraSettings) {
        *self.shared.settings.lock().unwrap() = *settings;
    }

    /// The next frame waiting, if there is one.
    pub fn next(&self) -> Option<Frame> {
        self.frames.try_recv().ok()
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

fn capture(shared: &Shared, source: &Source, sender: &Sender<Frame>) {
    let stopping = || shared.stop.load(Ordering::Relaxed);
    let set_state = |state| shared.status.lock().unwrap().state = state;
    while !stopping() {
        let settings = *shared.settings.lock().unwrap();
        let mut camera = match camera::open(source, &settings) {
            Ok(camera) => camera,
            Err(e) => {
                set_state(State::Problem(format!("waiting for camera: {e}")));
                let retry_at = Instant::now() + Duration::from_millis(500);
                while !stopping() && Instant::now() < retry_at {
                    std::thread::sleep(Duration::from_millis(20));
                }
                continue;
            }
        };
        {
            let mut status = shared.status.lock().unwrap();
            status.description = camera.description();
            status.state = State::Running;
        }
        let (mut applied, mut last) = (None, None::<u64>);
        let mut recent = VecDeque::new(); // camera timestamps within RATE_WINDOW
        while !stopping() {
            let settings = *shared.settings.lock().unwrap();
            if applied != Some(settings) {
                let mut warnings = camera.warnings();
                if let Err(e) = camera.apply(&settings) {
                    warnings.push(format!("Couldn't apply exposure/gain: {e}"));
                }
                let mut status = shared.status.lock().unwrap();
                status.warnings = warnings;
                status.controls = camera.controls();
                applied = Some(settings);
            }
            let frame = match camera.next_frame() {
                Ok(Some(frame)) => frame,
                Ok(None) => continue,
                Err(e) => {
                    set_state(State::Problem(format!("camera error, reconnecting: {e}")));
                    break;
                }
            };
            let (id, timestamp) = (frame.id, frame.timestamp);
            let queue_full = match sender.try_send(frame) {
                Ok(()) => false,
                Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Disconnected(_)) => return,
            };
            let mut status = shared.status.lock().unwrap();
            status.frames += 1;
            status.queue_dropped += queue_full as u64;
            if let Some(last) = last {
                status.camera_dropped += id.saturating_sub(last + 1);
            }
            last = Some(id);
            recent.push_back(timestamp);
            while recent.front().is_some_and(|&t| t < timestamp - RATE_WINDOW) {
                recent.pop_front();
            }
            let span = timestamp - recent.front().copied().unwrap_or(timestamp);
            if span > 0.0 {
                status.fps = (recent.len() - 1) as f64 / span;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;

    /// End to end with synthetic_streams/fake_camera.py, so needs python3: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn captures_from_the_fake_camera() {
        let port = 27398;
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/synthetic_streams/fake_camera.py");
        let mut child = Command::new("python3")
            .args([script, "--port", &port.to_string(), "--fps", "50"])
            .stdout(Stdio::null())
            .spawn()
            .expect("run python3");
        let source = Source::Fake { address: format!("127.0.0.1:{port}") };
        let capture = Capture::start(source, CameraSettings::default());
        let mut frames = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while frames.len() < 50 && Instant::now() < deadline {
            match capture.next() {
                Some(frame) => frames.push((frame.width, frame.height, frame.timestamp)),
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        let mut status = Status::default();
        capture.status_into(&mut status);
        drop(capture);
        child.kill().ok();
        child.wait().ok();
        assert_eq!(frames.len(), 50, "frames from the fake camera; status {status:?}");
        assert!(frames.iter().all(|&(w, h, _)| (w, h) == (640, 480)));
        let span = frames.last().unwrap().2 - frames[0].2;
        assert!((0.8..1.2).contains(&span), "50 frames at 50 fps should span about a second, got {span}");
    }
}
