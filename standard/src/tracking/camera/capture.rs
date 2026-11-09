//! Capturing from a camera on a thread of its own, so frames are collected as they arrive whatever the
//! passes are doing. Each frame's copied from the camera straight into a pool of a pass's video buffers, which the
//! Camera node's passes swap for the workers' own (see `crate::standard::pool`): each pass takes the
//! frames that have come since the last, and nothing more's copied. The thread's status (what it's doing, the frame
//! rate, drops) is shared for display. If the camera can't be opened, or fails, it keeps trying.
//!
//! Opening a camera takes a couple of seconds, so a new epoch doesn't: its `Capture` carries on with the camera that's
//! capturing (new state's made before the old's let go of), changing only the pool if the frame size has changed. It's
//! closed once nothing's using it, when the run stops. Two Camera nodes with the same source share it too.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{self as camera, CameraControls, CameraSettings, Source};
use crate::standard::pool::{Pool, SLACK_PASSES};
use crate::standard::tracking::{Video, VideoBuffer};
use ephys_rs::engine::{DataStreamType, PASS_DURATION};

/// About the most memory a camera's pool takes (it's at least a few passes' buffers, though).
const POOL_BYTES: usize = 32 << 20;
/// How long without a frame before the console's told.
const QUIET_AFTER: Duration = Duration::from_secs(5);

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
    /// Frames skipped for not being the size the passes expect, and the size they're coming at, if the last one was.
    pub wrong_size: u64,
    pub last_wrong_size: Option<[u32; 2]>,
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
    /// The video the passes expect, and the pool frames go into for them.
    target: Mutex<(Video, Pool<VideoBuffer>)>,
}

/// A camera capturing on its own thread, until the last `Capture` using it lets go.
struct Running {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Running {
    /// The pool for passes expecting `video`: the one in use, or if that's another size, a new one, from now on.
    fn pool_for(&self, video: Video) -> Pool<VideoBuffer> {
        let mut target = self.shared.target.lock().unwrap();
        if target.0 != video {
            *target = (video, new_pool(video));
            let mut status = self.shared.status.lock().unwrap();
            (status.wrong_size, status.last_wrong_size) = (0, None);
        }
        target.1.clone()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The cameras capturing, by source. Weak, so a camera closes once nothing's using it.
static RUNNING: Mutex<Vec<(Source, Weak<Running>)>> = Mutex::new(Vec::new());

fn new_pool(video: Video) -> Pool<VideoBuffer> {
    let bytes = video.buffer(PASS_DURATION).pixels.len();
    let size = (POOL_BYTES / bytes.max(1)).clamp(4, SLACK_PASSES);
    Pool::new((0..size).map(|_| video.buffer(PASS_DURATION)), bytes)
}

/// An epoch's use of a camera capturing.
pub struct Capture {
    running: Arc<Running>,
    pool: Pool<VideoBuffer>,
}

impl Capture {
    /// Uses `source` at `settings`, for passes' `video`: carrying on with it, if it's capturing already, otherwise
    /// opening it.
    pub fn start(source: Source, settings: CameraSettings, video: Video) -> Self {
        let mut running = RUNNING.lock().unwrap();
        running.retain(|(_, capture)| capture.strong_count() > 0);
        if let Some(capture) = running.iter().find(|(s, _)| *s == source).and_then(|(_, capture)| capture.upgrade()) {
            log::info!("Camera: carrying on with it, for {}×{} frames", video.width, video.height);
            let pool = capture.pool_for(video);
            return Capture { running: capture, pool };
        }
        let pool = new_pool(video);
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            settings: Mutex::new(settings),
            status: Mutex::new(Status::default()),
            target: Mutex::new((video, pool.clone())),
        });
        let (thread_shared, thread_source) = (shared.clone(), source.clone());
        let thread = std::thread::Builder::new()
            .name("camera capture".into())
            .spawn(move || capture(&thread_shared, &thread_source))
            .expect("capture thread");
        let capture = Arc::new(Running { shared, thread: Some(thread) });
        running.push((source, Arc::downgrade(&capture)));
        Capture { running: capture, pool }
    }

    /// Changes the frame rate, exposure and gain.
    pub fn apply(&self, settings: &CameraSettings) {
        *self.running.shared.settings.lock().unwrap() = *settings;
    }

    /// The buffers the passes swap with.
    pub fn pool(&self) -> &Pool<VideoBuffer> {
        &self.pool
    }

    /// The size frames are coming at, if it isn't the size the passes expect.
    pub fn wrong_size(&self) -> Option<[u32; 2]> {
        self.running.shared.status.lock().unwrap().last_wrong_size
    }

    /// Copies the status into `status`, reusing its memory.
    pub fn status_into(&self, status: &mut Status) {
        status.copy_from(&self.running.shared.status.lock().unwrap());
    }
}

/// What's been said about connecting, as it's tried again and again: each thing's said once, until it's connected.
#[derive(Default)]
struct Notes(HashSet<String>);

impl Notes {
    fn say(&mut self, text: String) {
        if !self.0.contains(&text) {
            ephys_rs::crash::note(&text);
            log::info!("{text}");
            self.0.insert(text);
        }
    }
}

fn capture(shared: &Shared, source: &Source) {
    let stopping = || shared.stop.load(Ordering::Relaxed);
    let set_state = |state: State| {
        let mut status = shared.status.lock().unwrap();
        // Logged when it changes, not every time it's tried again.
        if let (State::Problem(problem), true) = (&state, status.state != state) {
            log::warn!("{problem}");
        }
        status.state = state;
    };
    let mut notes = Notes::default();
    while !stopping() {
        let settings = *shared.settings.lock().unwrap();
        let mut camera = match camera::open(source, &settings, &mut |text| notes.say(text)) {
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
        notes = Notes::default();
        for warning in camera.warnings() {
            log::warn!("Camera: {warning}");
        }
        let (mut applied, mut last) = (None, None::<u64>);
        let (mut latest_frame, mut quiet) = (Instant::now(), false);
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
                Ok(None) => {
                    if !quiet && latest_frame.elapsed() > QUIET_AFTER {
                        log::warn!(
                            "Camera: no frames for {} s. If it's FLIR, check its TriggerMode, logged as it opened, isn't \
                             waiting for a trigger, and that nothing's said about incomplete frames",
                            QUIET_AFTER.as_secs()
                        );
                        quiet = true;
                    }
                    continue;
                }
                Err(e) => {
                    set_state(State::Problem(format!("camera error, reconnecting: {e}")));
                    break;
                }
            };
            let (id, timestamp) = (frame.id, frame.timestamp);
            if last.is_none() {
                log::info!("Camera: first frame, {}×{}", frame.width, frame.height);
            } else if quiet {
                log::info!("Camera: frames are coming again");
            }
            (latest_frame, quiet) = (Instant::now(), false);
            let mut expected = [0, 0];
            let pool = shared.target.lock().unwrap().1.clone();
            // Into the next slot of the buffer being filled, if it's the size the passes expect.
            let into_pool = pool.fill(
                |video| video.frames = 0,
                |video| {
                    if (frame.width as usize, frame.height as usize) != (video.width, video.height) {
                        expected = [video.width, video.height];
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
                    if status.last_wrong_size != Some([frame.width, frame.height]) {
                        let ([w, h], [set_w, set_h]) = ([frame.width, frame.height], expected);
                        log::warn!("Camera: frames are {w}×{h}, not the {set_w}×{set_h} set, so they're being skipped");
                    }
                    status.last_wrong_size = Some([frame.width, frame.height]);
                }
                None => {
                    pool.overflowed();
                    if status.not_taken == 0 {
                        log::error!("The passes aren't keeping up with the camera: frames are being lost");
                    }
                    status.not_taken += 1;
                }
                Some(Some(_)) => status.last_wrong_size = None,
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

    /// Takes from `capture`'s pool as passes do, until `wanted` frames or `seconds`: each frame's size and time.
    fn take_like_passes(capture: &Capture, wanted: usize, seconds: u64) -> Vec<(usize, usize, f64)> {
        let mut frames = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while frames.len() < wanted && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            // All that's waiting, as passes catching up would.
            while let Some(video) = capture.pool().take(|v| v.frames == 0) {
                frames.extend((0..video.frames).map(|i| (video.width, video.height, video.times[i])));
                capture.pool().give_back(video);
            }
        }
        frames
    }

    #[test]
    fn what_was_tried_is_said_once() {
        let mut notes = Notes::default();
        let source = Source::Fake { address: "127.0.0.1:1".into() };
        for _ in 0..3 {
            let _ = camera::open(&source, &CameraSettings::default(), &mut |text| notes.say(text));
        }
        assert_eq!(notes.0.len(), 1, "however many times it's tried");
        assert!(notes.0.iter().all(|text| text.contains("127.0.0.1:1")), "and says where");
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
        assert_eq!(status.last_wrong_size, Some([320, 240]));
    }

    #[test]
    fn epochs_carry_on_with_the_camera_and_the_last_closes_it() {
        // The test scene takes one connection, so frames coming later show it wasn't opened again.
        let source = Source::Fake { address: camera::serve_test_scene(320, 240) };
        let settings = CameraSettings::default();
        let (right, wrong) =
            (Video { width: 320, height: 240, fps: 50.0 }, Video { width: 640, height: 480, fps: 50.0 });
        let first = Capture::start(source.clone(), settings, wrong);
        let deadline = Instant::now() + Duration::from_secs(5);
        while first.wrong_size().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(first.wrong_size(), Some([320, 240]));

        // The next epoch, with the right size.
        let second = Capture::start(source.clone(), settings, right);
        assert!(Arc::ptr_eq(&first.running, &second.running), "the same camera");
        drop(first);
        assert!(take_like_passes(&second, 5, 5).len() >= 5, "its frames, in the new size");
        assert_eq!(second.wrong_size(), None);

        // Epochs of the same size share the pool, so neither loses frames when they change over.
        let third = Capture::start(source.clone(), settings, right);
        assert!(Arc::ptr_eq(&second.running, &third.running));
        drop(second);
        assert!(take_like_passes(&third, 5, 5).len() >= 5);

        // Another frame rate's set on the camera that's open, for buffers of its own.
        let (other_rate, slower) = (CameraSettings { frame_rate: 25.0, ..settings }, Video { fps: 25.0, ..right });
        let fourth = Capture::start(source, other_rate, slower);
        assert!(Arc::ptr_eq(&third.running, &fourth.running));
        drop(fourth);

        let camera = Arc::downgrade(&third.running);
        drop(third);
        assert!(camera.upgrade().is_none(), "closed, and its thread finished");
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
