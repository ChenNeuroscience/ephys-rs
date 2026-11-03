//! Capturing from a camera on a thread of its own, so frames are collected as they arrive whatever the
//! passes are doing. Each frame's copied from the camera straight into a pool of a pass's video buffers, which the
//! Camera node's passes swap for the workers' own (see `crate::standard_components::pool`): each pass takes the
//! frames that have come since the last, and nothing more's copied. The thread's status (what it's doing, the frame
//! rate, drops) is shared for display. If the camera can't be opened, or fails, it keeps trying.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{self as camera, CameraControls, CameraSettings, Source};
use crate::core::engine::{DataStreamType, PASS_DURATION};
use crate::standard_components::pool::{Pool, SLACK_PASSES};
use crate::standard_components::tracking::{Video, VideoBuffer};

/// About the most memory a camera's pool takes (it's at least a few passes' buffers, though).
const POOL_BYTES: usize = 32 << 20;
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
    /// Frames lost because the passes were so far behind there was no buffer to put them in.
    pub not_taken: u64,
    /// Frames skipped for not being the size the passes expect, and the size of the last of them.
    pub wrong_size: u64,
    pub last_wrong_size: [u32; 2],
    /// Frames per second, from the camera's timestamps over the last couple of seconds.
    pub fps: f64,
}

impl Status {
    /// Copies `other`, reusing this one's memory (for its text).
    pub fn copy_from(&mut self, other: &Status) {
        self.state.clone_from(&other.state);
        self.description.clone_from(&other.description);
        self.warnings.clone_from(&other.warnings);
        (self.controls, self.frames, self.camera_dropped, self.not_taken, self.fps) =
            (other.controls, other.frames, other.camera_dropped, other.not_taken, other.fps);
        (self.wrong_size, self.last_wrong_size) = (other.wrong_size, other.last_wrong_size);
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
    pool: Pool<VideoBuffer>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Starts opening `source` at `settings` (its frame rate fixed from now on), for passes' `video`.
    pub fn start(source: Source, settings: CameraSettings, video: Video) -> Self {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            settings: Mutex::new(settings),
            status: Mutex::new(Status::default()),
        });
        let bytes = video.buffer(PASS_DURATION).pixels.len();
        let size = (POOL_BYTES / bytes.max(1)).clamp(4, SLACK_PASSES);
        let pool = Pool::new((0..size).map(|_| video.buffer(PASS_DURATION)), bytes);
        let (thread_shared, thread_pool) = (shared.clone(), pool.clone());
        let thread = std::thread::Builder::new()
            .name("camera capture".into())
            .spawn(move || capture(&thread_shared, &source, &thread_pool))
            .expect("capture thread");
        Capture { shared, pool, thread: Some(thread) }
    }

    /// Changes the exposure and gain (the frame rate only takes effect on restarting).
    pub fn apply(&self, settings: &CameraSettings) {
        *self.shared.settings.lock().unwrap() = *settings;
    }

    /// The buffers the passes swap with.
    pub fn pool(&self) -> &Pool<VideoBuffer> {
        &self.pool
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

fn capture(shared: &Shared, source: &Source, pool: &Pool<VideoBuffer>) {
    let stopping = || shared.stop.load(Ordering::Relaxed);
    let set_state = |state: State| {
        let mut status = shared.status.lock().unwrap();
        // Logged when it changes, not every time it's tried again.
        if let (State::Problem(problem), true) = (&state, status.state != state) {
            log::warn!("{problem}");
        }
        status.state = state;
    };
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
            log::info!("Capturing from {}", status.description);
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
            let frame = match camera.fetch() {
                Ok(Some(frame)) => frame,
                Ok(None) => continue,
                Err(e) => {
                    set_state(State::Problem(format!("camera error, reconnecting: {e}")));
                    break;
                }
            };
            let (id, timestamp) = (frame.id, frame.timestamp);
            // Into the next slot of the buffer being filled, if it's the size the passes expect.
            let into_pool = pool.fill(
                |video| video.frames = 0,
                |video| {
                    if (frame.width as usize, frame.height as usize) != (video.width, video.height) {
                        return None;
                    }
                    let index = video.frames;
                    camera.copy_to(Some(video.frame_mut(index)));
                    (video.ids[index], video.times[index]) = (id, timestamp);
                    video.frames += 1;
                    Some(video.frames == video.capacity)
                },
            );
            match into_pool {
                Some(Some(full)) => {
                    if full {
                        pool.filled();
                    }
                }
                _ => camera.copy_to(None),
            }
            let mut status = shared.status.lock().unwrap();
            status.frames += 1;
            match into_pool {
                Some(None) => {
                    status.wrong_size += 1;
                    status.last_wrong_size = [frame.width, frame.height];
                }
                None => {
                    pool.overflowed();
                    if status.not_taken == 0 {
                        log::error!("The passes aren't keeping up with the camera: frames are being lost");
                    }
                    status.not_taken += 1;
                }
                Some(Some(_)) => {}
            }
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
    /// Takes from `capture`'s pool as passes do, until `wanted` frames or `seconds`: each frame's size and time.
    fn take_like_passes(capture: &Capture, wanted: usize, seconds: u64) -> Vec<(usize, usize, f64)> {
        let mut frames = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while frames.len() < wanted && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            let Some(video) = capture.pool().take(|v| v.frames == 0) else { continue };
            frames.extend((0..video.frames).map(|i| (video.width, video.height, video.times[i])));
            capture.pool().give_back(video);
        }
        frames
    }

    #[test]
    fn passes_take_frames_as_they_come() {
        let source = Source::Fake { address: camera::serve_test_scene(320, 240) };
        let video = Video { width: 320, height: 240, fps: 50.0 };
        let capture = Capture::start(source, CameraSettings::default(), video);
        let frames = take_like_passes(&capture, 25, 10);
        let mut status = Status::default();
        capture.status_into(&mut status);
        assert!(frames.len() >= 25, "a pass can take two at once; status {status:?}");
        assert!(frames.windows(2).all(|w| w[1].2 > w[0].2), "in order");
        assert_eq!((status.not_taken, status.wrong_size), (0, 0));
        // Frames of another size are skipped.
        let source = Source::Fake { address: camera::serve_test_scene(320, 240) };
        let capture = Capture::start(source, CameraSettings::default(), Video { width: 640, ..video });
        assert!(take_like_passes(&capture, 1, 1).is_empty());
        capture.status_into(&mut status);
        assert_eq!(status.last_wrong_size, [320, 240]);
    }

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
        let video = Video { width: 640, height: 480, fps: 50.0 };
        let capture = Capture::start(source, CameraSettings::default(), video);
        let frames = take_like_passes(&capture, 50, 10);
        let mut status = Status::default();
        capture.status_into(&mut status);
        drop(capture);
        child.kill().ok();
        child.wait().ok();
        assert_eq!(frames.len(), 50, "frames from the fake camera; status {status:?}");
        let span = frames.last().unwrap().2 - frames[0].2;
        assert!((0.8..1.2).contains(&span), "50 frames at 50 fps should span about a second, got {span}");
    }
}
