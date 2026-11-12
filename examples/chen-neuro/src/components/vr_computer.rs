//! The VR Computer: positions, TTLs, log lines and settings from the computer running a virtual reality task, instead
//! of a camera's. It connects to the VR computer over TCP, keeping trying, and reads a JSON object a line (see
//! VR_COMPUTER_PROTOCOL.md, in this build's directory, which is to be kept up to date with this):
//!
//! ```json
//! {"type": "settings", "settings": {"window_min_x": 0, "window_max_x": 800, "window_min_y": 0, "window_max_y": 600,
//!                                    "pixels_per_metre": 400, "ttl_channels": 2,
//!                                    "maze": {"arms": 8}, "task": "linear"}}
//! {"type": "ttl", "time": 1520.25, "idx": 1, "value": 1}
//! {"type": "log", "time": 1520.25, "line": "trial 3 started"}
//! {"type": "pos", "time": 1520.26, "x": 412.5, "y": 87.0}
//! ```
//!
//! `settings` is a complete snapshot of the custom settings Write LTX records with the positions, nested keys joined
//! with dots (`maze.arms`), replacing the last. Positions are as a camera's would be, in pixels of a made-up frame, so
//! that what's recorded suits whatever reads a camera's. So settings have to say the window they range over, the pixels
//! in a metre, and how many TTL channels there are (see `WINDOW_KEYS`, `CALIBRATION_KEY` and `TTL_CHANNELS_KEY`), which
//! go to Write LTX as a crop, a ruler and a timing LED (if there are any channels), named as a camera's settings have
//! them: otherwise they're refused, and an error logged. The VR computer should send them on being connected to, and
//! whenever they change. They can't change while recording, as they're in headers already: ones that differ log an
//! error, and wait until it stops, so the node ends up with the latest it was sent. `ttl` is channel `idx` (from 1, as
//! the LTX files number them) going up (1) or down (0). `log` is a line of text, cut short to fit the log stream. `pos`
//! is a position within the window, as the first LED, with `AREA` for its size, and no second LED.
//!
//! It sends the VR computer a line too, its `state`, every `STATE_EVERY` and as recording starts and stops: how long
//! it's been acquiring, and recording, if it is, for the VR computer to show.
//!
//! `time` is in seconds by the VR computer's own clock, which has to be consistent for as long as it's connected: a line
//! without one isn't understood. It's kept as it is, for what's downstream: Write LTX counts from the first position
//! recorded. Messages arrive on a thread of their own, which carries on from epoch to epoch, like a camera's capture, so
//! nothing's lost by reconnecting.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::num::NonZeroU8;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::standard::ephys::{Event, Events, EventsBuffer};
use crate::standard::logs::{LogLines, LogLinesBuffer};
use crate::standard::pool::Pool;
use crate::standard::tracking::pipeline::Blob;
use crate::standard::tracking::{Crop, Detection, PositionSample, Positions, PositionsBuffer, Ruler, TimingLed};
use crate::standard::widgets::{named_settings_grid, setting};
use crate::standard::{Buffer, DataStream, Edges, Kind, Setting};
use eframe::egui::{self, RichText};
use ephys_rs::engine::{
    Category, Flow, Node, NodeInfo, NodeThread, Output, Pass, SettingPort, ThreadContext, UiContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Its name in headers (see `Positions::source`).
const SOURCE: &str = "vr_computer";
/// What's recorded as the size of the LED at each position: there isn't one, but the format has room for it.
const AREA: u32 = 1;
/// The settings saying the window positions range over.
const WINDOW_KEYS: [&str; 4] = ["window_min_x", "window_max_x", "window_min_y", "window_max_y"];
/// The setting saying how many of the positions' pixels there are in a metre.
const CALIBRATION_KEY: &str = "pixels_per_metre";
/// The setting saying how many TTL channels there are, each `idx` from 1.
const TTL_CHANNELS_KEY: &str = "ttl_channels";
/// The most TTL channels there can be, as what shows them keeps a bit for each.
const MOST_TTL_CHANNELS: u32 = 32;
/// The frame the positions are declared in until there's a window.
const FRAME_SIZE: u32 = 1000;
/// The rate the TTLs are declared at, which only matters to what goes by their samples: they carry their times.
const TTL_RATE: f64 = 1000.0;
/// The most of each kind of message kept for the passes: any more are lost.
const QUEUE: usize = 4096;
/// The longest message: longer than this, and it isn't one.
const MAX_MESSAGE: usize = 1 << 20;
/// How long to give the GUI to apply settings, before triggering an update to them again (it won't as recording starts).
const TRIGGER_AGAIN: Duration = Duration::from_secs(1);
/// The longest what's arrived waits for new settings to be applied, for passes that have them (see `Inbox::waiting`).
const WAIT_FOR_SETTINGS: Duration = Duration::from_secs(3);
/// How long to try each connection for, and to wait between them.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(250);
const RETRY_AFTER: Duration = Duration::from_millis(500);
/// How often the thread looks up from reading, to see if it should stop, or send its state.
const READ_TIMEOUT: Duration = Duration::from_millis(50);
/// How often it sends its state, as well as when recording starts or stops.
const STATE_EVERY: Duration = Duration::from_secs(1);
/// How long a state can take to send: any longer, and the VR computer isn't reading them, so they stop.
const WRITE_TIMEOUT: Duration = Duration::from_millis(200);

/// Custom settings, by name.
type Custom = BTreeMap<String, String>;
/// An IP address (or host name) and port.
type Address = (String, u16);

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VrComputer {
    pub ip: String,
    pub port: u16,
    /// Positions a second to expect, for the stream's rate.
    pub fps: f32,
    /// The most bytes in a log line.
    pub log_width: u32,
    /// The latest settings it sent.
    pub custom: Custom,
}

impl Default for VrComputer {
    fn default() -> Self {
        VrComputer { ip: "127.0.0.1".into(), port: 27032, fps: 50.0, log_width: 256, custom: Custom::new() }
    }
}

/// A line from the VR computer.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Message {
    Settings { settings: Map<String, Value> },
    Ttl { time: f64, idx: NonZeroU8, value: u8 },
    Log { time: f64, line: String },
    Pos { time: f64, x: f32, y: f32 },
}

/// `settings` as custom settings, with nested keys joined with dots.
fn flatten(settings: &Map<String, Value>) -> Custom {
    fn add(custom: &mut Custom, prefix: &str, settings: &Map<String, Value>) {
        for (key, value) in settings {
            let key = format!("{prefix}{key}");
            match value {
                Value::Object(nested) => add(custom, &format!("{key}."), nested),
                Value::String(text) => drop(custom.insert(key, text.clone())),
                other => drop(custom.insert(key, other.to_string())),
            }
        }
    }
    let mut custom = Custom::new();
    add(&mut custom, "", settings);
    custom
}

/// The window custom settings say positions range over, as a crop, if they say.
fn window(custom: &Custom) -> Option<Crop> {
    let [min_x, max_x, min_y, max_y] = WINDOW_KEYS.map(|key| custom.get(key)?.parse::<f32>().ok());
    let (min_x, max_x, min_y, max_y) = (min_x?, max_x?, min_y?, max_y?);
    let (width, height) = (max_x - min_x, max_y - min_y);
    let usable = width > 0.0 && height > 0.0 && width.is_finite() && height.is_finite();
    usable.then_some(Crop::Rectangle { center: [min_x + width / 2.0, min_y + height / 2.0], width, height, angle: 0.0 })
}

/// How many pixels there are in a metre, if custom settings say.
fn calibration(custom: &Custom) -> Option<f32> {
    custom.get(CALIBRATION_KEY)?.parse().ok().filter(|ppm: &f32| *ppm > 0.0 && ppm.is_finite())
}

/// How many TTL channels there are, if custom settings say.
fn ttl_channels(custom: &Custom) -> Option<u32> {
    custom.get(TTL_CHANNELS_KEY)?.parse().ok().filter(|n| *n <= MOST_TTL_CHANNELS)
}

/// Whether custom settings say all they have to.
fn complete(custom: &Custom) -> bool {
    window(custom).is_some() && calibration(custom).is_some() && ttl_channels(custom).is_some()
}

/// What's arrived since the passes last took any.
#[derive(Default)]
struct Inbox {
    positions: VecDeque<PositionSample>,
    ttls: VecDeque<Event>,
    logs: VecDeque<(f64, String)>,
    /// The latest settings, until the node has them.
    settings: Option<Custom>,
    /// When an update to them was last triggered, and whether it's been said they're waiting for recording to stop.
    triggered: Option<Instant>,
    held: bool,
    frames: u64,
}

impl Inbox {
    /// Files a message. Returns whether there was room.
    fn receive(&mut self, message: Message) -> bool {
        match message {
            Message::Settings { settings } => {
                let settings = flatten(&settings);
                if self.settings.as_ref() != Some(&settings) {
                    (self.triggered, self.held) = (None, false);
                }
                self.settings = Some(settings);
            }
            Message::Ttl { time, idx, value } => {
                if self.ttls.len() == QUEUE {
                    return false;
                }
                let line = idx.get() - 1;
                self.ttls.push_back(Event { sample: 0, line, high: value != 0, time: Some(time) });
            }
            Message::Log { time, line } => {
                if self.logs.len() == QUEUE {
                    return false;
                }
                self.logs.push_back((time, line));
            }
            Message::Pos { time, x, y } => {
                if self.positions.len() == QUEUE {
                    return false;
                }
                self.positions.push_back(PositionSample {
                    frame: self.frames,
                    time,
                    leds: [Some(Blob { x, y, area: AREA }), None],
                    timing_led: None,
                    pixels_per_metre: f32::NAN,
                });
                self.frames += 1;
            }
        }
        true
    }

    /// Moves as much as a pass has room for into its buffers, leaving the rest for the next.
    fn take(&mut self, positions: &mut PositionsBuffer, ttls: &mut EventsBuffer, logs: &mut LogLinesBuffer) {
        let room = positions.samples.capacity() - positions.samples.len();
        let count = self.positions.len().min(room);
        positions.samples.extend(self.positions.drain(..count));
        let room = ttls.events.capacity() - ttls.events.len();
        let count = self.ttls.len().min(room);
        ttls.events.extend(self.ttls.drain(..count));
        while !logs.is_full() {
            let Some((time, line)) = self.logs.pop_front() else { break };
            logs.push(time, &line);
        }
    }

    /// Whether what's arrived should wait for the passes of the new settings, not be taken by these (with `current`):
    /// while they're on their way (it's not `recording`, when they'd wait for it to stop), for a while at most. What
    /// came after them is by them (a TTL on a channel only they have, say), and the passes judge it by theirs.
    fn waiting(&self, current: &Custom, recording: bool) -> bool {
        let on_their_way = self.settings.as_ref().is_some_and(|settings| settings != current) && !recording;
        on_their_way && self.triggered.is_none_or(|at| at.elapsed() < WAIT_FOR_SETTINGS)
    }

    /// The settings to update a node to that has `current`, if they're new: kept until it has them. While `recording`
    /// they wait, with an error logged.
    fn settings_to_update(&mut self, current: &Custom, recording: bool) -> Option<Custom> {
        let settings = self.settings.as_ref()?;
        if settings == current {
            (self.settings, self.triggered, self.held) = (None, None, false);
            return None;
        }
        if recording {
            if !std::mem::replace(&mut self.held, true) {
                log::error!(
                    "VR Computer: its settings changed while recording: this recording keeps the ones from when it \
                     started, and the new ones apply when it stops"
                );
            }
            return None;
        }
        if self.triggered.is_some_and(|at| at.elapsed() < TRIGGER_AGAIN) {
            return None;
        }
        self.triggered = Some(Instant::now());
        Some(settings.clone())
    }
}

/// How the connection's going.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub connected: bool,
    /// Why it isn't, if it's tried.
    pub problem: Option<String>,
    pub messages: u64,
    /// Lines that weren't messages.
    pub not_understood: u64,
    /// Settings refused for not saying all they have to.
    pub refused: u64,
    /// Messages lost because the passes were so far behind there was no room for them.
    pub not_taken: u64,
}

struct Shared {
    stop: AtomicBool,
    inbox: Mutex<Inbox>,
    status: Mutex<Status>,
    /// How long it's been acquiring, and recording, as the last pass said (see `Pass::acquiring_for`), for its state.
    clock: Mutex<Clock>,
}

/// How long it's been acquiring, and recording, in seconds, as the last pass said.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Clock {
    /// None before the first pass.
    acquiring: Option<f64>,
    /// None while it isn't recording.
    recording: Option<f64>,
}

impl Clock {
    /// The state message saying so, if there's been a pass.
    fn message(&self) -> Option<String> {
        let ms = |seconds: f64| (seconds * 1000.0).round() / 1000.0;
        let acquiring = self.acquiring?;
        let recording = self.recording.map(ms);
        let state = serde_json::json!({"type": "state", "acquiring_for": ms(acquiring), "recording_for": recording});
        Some(format!("{state}\n"))
    }
}

impl Shared {
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Handles a line of text, saying the first time in a connection one can't be understood.
    fn line(&self, line: &[u8], complained: &mut bool) {
        if line.trim_ascii().is_empty() {
            return;
        }
        let mut status = self.status.lock().unwrap();
        match serde_json::from_slice::<Message>(line) {
            Ok(Message::Settings { settings }) if !complete(&flatten(&settings)) => {
                status.refused += 1;
                let keys = WINDOW_KEYS.join(", ");
                log::error!(
                    "VR Computer: settings refused: they have to have numbers for {keys} (with min below max), a \
                     positive {CALIBRATION_KEY}, and a whole number up to {MOST_TTL_CHANNELS} for {TTL_CHANNELS_KEY}"
                );
            }
            Ok(message) => {
                status.messages += 1;
                if !self.inbox.lock().unwrap().receive(message) {
                    status.not_taken += 1;
                }
            }
            Err(e) => {
                status.not_understood += 1;
                if !std::mem::replace(complained, true) {
                    log::warn!("VR Computer: didn't understand {}: {e}", String::from_utf8_lossy(line).trim_end());
                }
            }
        }
    }

    /// Sends the state, if it's due: every `STATE_EVERY`, and as soon as recording starts or stops. Once one's
    /// taken too long to send, it stops for the connection, as the VR computer isn't reading them.
    fn send_state(&self, mut stream: &TcpStream, sent: &mut Sent) {
        if sent.stalled {
            return;
        }
        let clock = *self.clock.lock().unwrap();
        let recording = clock.recording.is_some();
        let due = sent.at.is_none_or(|at| at.elapsed() >= STATE_EVERY) || sent.recording != recording;
        let Some(message) = clock.message().filter(|_| due) else { return };
        if let Err(e) = stream.write_all(message.as_bytes()) {
            log::warn!("VR Computer: stopped sending it its state, as it isn't reading them ({e})");
            sent.stalled = true;
        }
        (sent.at, sent.recording) = (Some(Instant::now()), recording);
    }

    /// Reads messages until told to stop, or the connection ends, sending the state as it goes.
    fn read(&self, stream: TcpStream) -> std::io::Result<()> {
        let mut reader = BufReader::new(stream);
        let (mut line, mut complained, mut sent) = (Vec::new(), false, Sent::default());
        while !self.stopping() {
            self.send_state(reader.get_ref(), &mut sent);
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
                Ok(_) if line.ends_with(b"\n") => {
                    self.line(&line, &mut complained);
                    line.clear();
                }
                Ok(_) => return Err(ErrorKind::UnexpectedEof.into()),
                // What's come of the line so far stays in it, for the rest.
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    if line.len() > MAX_MESSAGE {
                        return Err(std::io::Error::new(ErrorKind::InvalidData, "a message was too long"));
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

/// The state sent on a connection: when last, whether it said it was recording, and whether sending stalled.
#[derive(Default)]
struct Sent {
    at: Option<Instant>,
    recording: bool,
    stalled: bool,
}

/// Connects, with latency kept down as far as it can be (though it can't be relied on).
fn connect(address: &Address) -> std::io::Result<TcpStream> {
    let found = address.to_socket_addrs()?.next();
    let found = found.ok_or_else(|| std::io::Error::new(ErrorKind::NotFound, "no such address"))?;
    let stream = TcpStream::connect_timeout(&found, CONNECT_TIMEOUT)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    Ok(stream)
}

/// The thread: connects, reads until the connection ends, and tries again, until told to stop.
fn receive(shared: &Shared, address: &Address) {
    let shown = format!("{}:{}", address.0, address.1);
    while !shared.stopping() {
        let ended = match connect(address) {
            Ok(stream) => {
                log::info!("VR Computer: connected to {shown}");
                let mut status = shared.status.lock().unwrap();
                (status.connected, status.problem) = (true, None);
                drop(status);
                shared.read(stream)
            }
            Err(e) => Err(e),
        };
        let Err(e) = ended else { continue };
        let problem = format!("waiting for {shown}: {e}");
        let mut status = shared.status.lock().unwrap();
        // Logged when it changes, not every time it's tried again.
        if status.problem.as_ref() != Some(&problem) {
            log::warn!("VR Computer: {problem}");
        }
        (status.connected, status.problem) = (false, Some(problem));
        drop(status);
        let retry_at = Instant::now() + RETRY_AFTER;
        while !shared.stopping() && Instant::now() < retry_at {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A connection on its own thread, until the last `Connection` using it lets go.
struct Running {
    shared: Arc<Shared>,
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

/// The connections open, by address. Weak, so one closes once nothing's using it.
static RUNNING: Mutex<Vec<(Address, Weak<Running>)>> = Mutex::new(Vec::new());

/// An epoch's use of a connection.
struct Connection {
    address: Address,
    running: Arc<Running>,
}

impl Connection {
    /// Uses the connection to `address`, carrying on with it if there is one.
    fn start(address: Address) -> Self {
        let mut running = RUNNING.lock().unwrap();
        running.retain(|(_, connection)| connection.strong_count() > 0);
        let open = running.iter().find(|(a, _)| *a == address).and_then(|(_, connection)| connection.upgrade());
        if let Some(open) = open {
            return Connection { address, running: open };
        }
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            inbox: Mutex::default(),
            status: Mutex::default(),
            clock: Mutex::default(),
        });
        let (thread_shared, thread_address) = (shared.clone(), address.clone());
        let thread = std::thread::Builder::new()
            .name("VR computer".into())
            .spawn(move || receive(&thread_shared, &thread_address))
            .expect("VR computer thread");
        let connection = Arc::new(Running { shared, thread: Some(thread) });
        running.push((address.clone(), Arc::downgrade(&connection)));
        Connection { address, running: connection }
    }

    fn is_to(&self, ip: &str, port: u16) -> bool {
        self.address.0 == ip && self.address.1 == port
    }
}

/// While running: the connection, and its status for the window.
#[derive(Default)]
pub struct Receiving {
    connection: Option<Connection>,
    /// Whether it's been said that TTLs are coming on channels settings don't have.
    told: bool,
}

impl ephys_rs::engine::PassState for Receiving {
    const WHY: &'static str = "The connection to the VR computer: each pass takes the messages that have come since \
                               the last, in order.";
}

impl Node<Edges> for VrComputer {
    const INFO: NodeInfo = NodeInfo {
        name: "VR Computer",
        category: Category::Source,
        description: "Positions, TTLs, log lines and settings from the computer running a VR task, over TCP as a JSON \
                      object a line.",
    };

    type State = Receiving;
    /// What it shows, for its window.
    type Shared = Pool<Status>;
    type UiState = ();
    type GlobalSettings = ();

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        let [_, _, width, height] = window(&self.custom).and_then(|crop| crop.bounds()).unwrap_or([0.0; 4]);
        let frame = |size: f32| if size > 0.0 { size.ceil() as u32 } else { FRAME_SIZE };
        let positions = Positions { fps: self.fps, width: frame(width), height: frame(height), source: SOURCE };
        let ttl = Events { lines: ttl_channels(&self.custom).unwrap_or(0), sample_rate: TTL_RATE as f32 };
        vec![
            Output { name: "positions", kind: Kind::Positions, stream: Some(DataStream::Positions(positions)) },
            Output { name: "pos ttl", kind: Kind::Events, stream: Some(DataStream::Events(ttl)) },
            Output {
                name: "log",
                kind: Kind::LogLines,
                stream: Some(DataStream::LogLines(LogLines { width: self.log_width })),
            },
        ]
    }

    fn warnings(&self, _inputs: &[Option<DataStream>]) -> Vec<String> {
        if complete(&self.custom) {
            return Vec::new();
        }
        vec!["No settings from the VR computer yet: positions can't be recorded properly without them".into()]
    }

    fn settings(&self) -> Vec<SettingPort<Edges>> {
        let crop = window(&self.custom).unwrap_or_default();
        // A metre's worth of pixels, as a ruler laid over the video would say.
        let ruler = |ppm| Ruler { a: [0.0, 0.0], b: [ppm, 0.0], length_cm: 100.0 };
        let rulers = calibration(&self.custom).map(ruler).into_iter().collect();
        // What's gone as those: as custom settings, they'd clash with the headers Write LTX makes of them.
        let required = |key: &str| WINDOW_KEYS.contains(&key) || key == CALIBRATION_KEY || key == TTL_CHANNELS_KEY;
        let others = self.custom.iter().filter(|(key, _)| !required(key));
        let custom = others.map(|(key, value)| (key.clone(), value.clone())).collect();
        // As a timing LED, if there are TTLs, though there's no spot in any video to say where it is: Write LTX records
        // it as it is.
        let enabled = ttl_channels(&self.custom).is_some_and(|n| n > 0);
        let timing_led = TimingLed { enabled, radius: 0.0, ..Default::default() };
        let detection = Detection { crop, rulers, timing_led, custom, ..Default::default() };
        vec![SettingPort { name: "settings", value: Setting::Detection(detection), on_right: true }]
    }

    fn set_setting(&mut self, _index: usize, value: Setting) {
        if let Setting::Detection(detection) = value {
            self.custom = detection.custom;
        }
    }

    fn threads(&self) -> Vec<NodeThread> {
        vec![NodeThread {
            name: "VR computer",
            link: "message queue",
            flow: Flow::ToPasses,
            why: "On a thread of its own so messages are read as they come, whatever the passes are doing.",
            swapped_outputs: Vec::new(),
        }]
    }

    fn thread_ui(&self, _thread: usize, ui: &mut egui::Ui, context: &ThreadContext) {
        let pool = context.shared::<Pool<Status>>();
        match pool.and_then(Pool::newest) {
            Some(status) => status_ui(ui, &status, context.theme.warning),
            None => drop(ui.weak("Reads messages as they arrive; starts when running.")),
        }
    }

    fn initial_state(&self, _inputs: &[Option<DataStream>]) -> Receiving {
        Receiving { connection: Some(Connection::start((self.ip.clone(), self.port))), ..Default::default() }
    }

    fn process(&self, pass: &mut Pass<Edges>, receiving: &mut Receiving) {
        // Changing the address doesn't start a new epoch.
        if !receiving.connection.as_ref().is_some_and(|c| c.is_to(&self.ip, self.port)) {
            receiving.connection = Some(Connection::start((self.ip.clone(), self.port)));
        }
        let Some(connection) = &receiving.connection else { return };
        let shared = &connection.running.shared;
        let recording = pass.recording();
        // Its state, for the VR computer.
        *shared.clock.lock().unwrap() = Clock {
            acquiring: Some(pass.acquiring_for().as_secs_f64()),
            recording: pass.recording_for().map(|d| d.as_secs_f64()),
        };
        let [Buffer::Positions(positions), Buffer::Events(ttls), Buffer::LogLines(logs)] = &mut *pass.outputs else {
            return;
        };
        positions.samples.clear();
        ttls.events.clear();
        ttls.pass_samples = (pass.duration * TTL_RATE).round() as usize;
        logs.clear();
        // If the thread's filing a message just then, what's there is left for the next pass; and while new settings
        // are on their way, for the passes that have them.
        let settings = shared.inbox.try_lock().ok().and_then(|mut inbox| {
            if !inbox.waiting(&self.custom, recording) {
                inbox.take(positions, ttls, logs);
            }
            inbox.settings_to_update(&self.custom, recording)
        });
        let channels = ttl_channels(&self.custom).unwrap_or(0);
        let beyond = keep_channels(&mut ttls.events, channels);
        if let Some(idx) = beyond.filter(|_| !std::mem::replace(&mut receiving.told, true)) {
            log::error!(
                "VR Computer: TTLs are coming on channel {idx}, but its settings say {TTL_CHANNELS_KEY} is {channels}, \
                 so those beyond it won't be recorded"
            );
        }
        let pixels_per_metre = calibration(&self.custom).unwrap_or(f32::NAN);
        for sample in &mut positions.samples {
            sample.pixels_per_metre = pixels_per_metre;
        }
        if let Some(custom) = settings {
            pass.trigger_settings_update(0, Setting::Detection(Detection { custom, ..Default::default() }));
        }
        let status = &shared.status;
        pass.shared::<Pool<Status>>().send_newest(pass.sequence, |shown| shown.clone_from(&status.lock().unwrap()));
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, context| {
            named_settings_grid(ui, "vr computer", |ui| {
                setting(ui, "IP address", |ui| ui.text_edit_singleline(&mut node.ip));
                setting(ui, "Port", |ui| ui.add(egui::DragValue::new(&mut node.port)));
                setting(ui, "Position rate", |ui| {
                    ui.add(egui::DragValue::new(&mut node.fps).range(1.0..=1000.0).suffix(" Hz"))
                        .on_hover_text("How many positions to expect a second, as a frame rate: the alarms go by it")
                });
                setting(ui, "Log line limit", |ui| {
                    ui.add(egui::DragValue::new(&mut node.log_width).range(16..=65536).suffix(" bytes"))
                        .on_hover_text("Longer log lines are cut short")
                });
            });
            ui.separator();
            let pool = context.shared::<Pool<Status>>();
            match pool.and_then(Pool::newest) {
                Some(status) => status_ui(ui, &status, context.theme.warning),
                None => drop(ui.weak("Press Play to connect.")),
            }
            ui.add_space(4.0);
            if node.custom.is_empty() {
                ui.weak("No settings from the VR computer yet.");
                return;
            }
            ui.label(RichText::new("Settings from the VR computer").strong());
            egui::Grid::new("vr settings").num_columns(2).striped(true).show(ui, |ui| {
                for (key, value) in &node.custom {
                    ui.label(key);
                    ui.label(value);
                    ui.end_row();
                }
            });
        });
    }
}

/// Keeps the TTLs on the first `channels`: returns the `idx` of the first that wasn't, if any.
fn keep_channels(events: &mut Vec<Event>, channels: u32) -> Option<u32> {
    let mut beyond = None;
    events.retain(|event| {
        let kept = u32::from(event.line) < channels;
        beyond = beyond.or((!kept).then_some(u32::from(event.line) + 1));
        kept
    });
    beyond
}

fn status_ui(ui: &mut egui::Ui, status: &Status, warning: egui::Color32) {
    match &status.problem {
        Some(problem) => drop(ui.label(RichText::new(problem).color(warning))),
        None if status.connected => drop(ui.label("Connected")),
        None => drop(ui.weak("Connecting…")),
    }
    ui.weak(format!("{} messages", status.messages));
    if status.not_understood + status.not_taken > 0 {
        let text = format!("{} not understood, {} not taken in time", status.not_understood, status.not_taken);
        ui.label(RichText::new(text).color(warning));
    }
    if status.refused > 0 {
        ui.label(
            RichText::new(format!("{} settings refused, for not saying all they have to", status.refused))
                .color(warning),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephys_rs::engine::{DataStreamType, PASS_DURATION};
    use std::io::Write;
    use std::net::TcpListener;

    fn parse(line: &str) -> Message {
        serde_json::from_str(line).unwrap()
    }

    /// A pass's buffers.
    fn buffers(node: &VrComputer) -> (PositionsBuffer, EventsBuffer, LogLinesBuffer) {
        let outputs = node.outputs(&[]);
        let (Some(DataStream::Positions(p)), Some(DataStream::Events(e)), Some(DataStream::LogLines(l))) =
            (outputs[0].stream, outputs[1].stream, outputs[2].stream)
        else {
            panic!("its streams")
        };
        (p.buffer(PASS_DURATION), e.buffer(PASS_DURATION), l.buffer(PASS_DURATION))
    }

    #[test]
    fn what_the_protocol_document_says_about_edge_cases() {
        let ok = |line: &str| serde_json::from_str::<Message>(line).is_ok();
        assert!(
            ok(r#"{"type": "ttl", "time": 1, "idx": 1, "value": 1, "extra": "ignored"}"#),
            "other fields are ignored"
        );
        assert!(ok(r#"{"time": 1, "value": 0, "idx": 2, "type": "ttl"}"#), "in any order");
        assert!(!ok(r#"{"type": "ttl", "time": 1, "idx": 1, "value": true}"#), "a number");
        assert!(!ok(r#"{"type": "ttl", "time": 1, "idx": 1, "value": 1.0}"#), "a whole number");
        assert!(!ok(r#"{"type": "ttl", "time": 1, "value": 1}"#), "an idx");
        assert!(!ok(r#"{"type": "ttl", "time": 1, "idx": 0, "value": 1}"#), "from 1");
        assert!(!ok(r#"{"type": "ttl", "time": 1, "idx": 256, "value": 1}"#), "up to 255");
        assert!(!ok(r#"{"type": "pos", "time": "1", "x": 1, "y": 2}"#), "a number, not text");
        assert!(!ok(r#"{"type": "pos", "time": 1, "x": null, "y": 2}"#), "no nulls for a missing position");
        let Message::Settings { settings } = parse(r#"{"type": "settings", "settings": {"a": null, "b": {"c": [1]}}}"#)
        else {
            panic!()
        };
        assert_eq!(flatten(&settings).get("a").map(String::as_str), Some("null"));
        assert_eq!(flatten(&settings).get("b.c").map(String::as_str), Some("[1]"));
        let nested = Custom::from([("view.window_min_x".to_string(), "0".to_string())]);
        assert!(!complete(&nested), "the required settings are at the top level");
    }

    #[test]
    fn the_messages_are_understood() {
        let ttl = parse(r#"{"type": "ttl", "time": 1, "idx": 3, "value": 1}"#);
        assert_eq!(ttl, Message::Ttl { time: 1.0, idx: NonZeroU8::new(3).unwrap(), value: 1 });
        let pos = parse(r#"{"type": "pos", "time": 2.5, "x": 1.5, "y": 2}"#);
        assert_eq!(pos, Message::Pos { time: 2.5, x: 1.5, y: 2.0 });
        let log = parse(r#"{"type": "log", "time": 3, "line": "hi"}"#);
        assert_eq!(log, Message::Log { time: 3.0, line: "hi".into() });
        assert!(serde_json::from_str::<Message>(r#"{"type": "other"}"#).is_err());
        let untimed = r#"{"type": "ttl", "idx": 1, "value": 1}"#;
        assert!(serde_json::from_str::<Message>(untimed).is_err(), "they have to have a time");
    }

    #[test]
    fn nested_settings_are_joined_with_dots() {
        let line = r#"{"type": "settings", "settings": {"maze": {"arms": 8, "wall": {"on": true}}, "task": "linear",
                       "list": [1, 2]}}"#;
        let Message::Settings { settings } = parse(line) else { panic!() };
        let custom = flatten(&settings);
        let pairs: Vec<(&str, &str)> = custom.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(
            pairs,
            [("list", "[1,2]"), ("maze.arms", "8"), ("maze.wall.on", "true"), ("task", "linear")],
            "text as it is, without quotes"
        );
    }

    #[test]
    fn settings_have_to_say_the_window_and_calibration() {
        let custom =
            |pairs: &[(&str, &str)]| -> Custom { pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() };
        let keys = |values: [&str; 4]| custom(&std::array::from_fn::<_, 4, _>(|i| (WINDOW_KEYS[i], values[i])));
        let crop = Crop::Rectangle { center: [0.0, 1.0], width: 10.0, height: 4.0, angle: 0.0 };
        assert_eq!(window(&keys(["-5", "5", "-1", "3"])), Some(crop));
        assert_eq!(window(&custom(&[("task", "linear")])), None, "missing");
        assert_eq!(window(&keys(["0", "text", "0", "1"])), None, "not a number");
        assert_eq!(window(&keys(["5", "5", "0", "1"])), None, "empty");
        assert_eq!(window(&keys(["5", "-5", "0", "1"])), None, "backwards");

        let node = VrComputer::default();
        assert_eq!(node.warnings(&[]).len(), 1);
        let mut custom = keys(["-5", "5", "-1", "3"]);
        assert!(!complete(&custom), "no calibration");
        custom.insert(CALIBRATION_KEY.into(), "0".into());
        assert!(!complete(&custom), "no use");
        custom.insert(CALIBRATION_KEY.into(), "250".into());
        assert!(!complete(&custom), "not saying how many TTL channels");
        for refused in ["true", "-1", "1.5", "33"] {
            custom.insert(TTL_CHANNELS_KEY.into(), refused.into());
            assert!(!complete(&custom), "{refused}: a whole number, up to 32");
        }
        custom.insert(TTL_CHANNELS_KEY.into(), "0".into());
        assert!(complete(&custom), "none");
        let node = VrComputer { custom: custom.clone(), ..VrComputer::default() };
        let [SettingPort { value: Setting::Detection(detection), .. }] = &node.settings()[..] else { panic!() };
        assert!(!detection.timing_led.enabled, "no TTLs");
        custom.insert(TTL_CHANNELS_KEY.into(), "3".into());
        assert!(complete(&custom));
        let node = VrComputer { custom, ..node };
        assert!(node.warnings(&[]).is_empty());
        let Some(DataStream::Events(ttl)) = node.outputs(&[])[1].stream else { panic!() };
        assert_eq!(ttl.lines, 3, "a line a channel");
        let [SettingPort { value: Setting::Detection(detection), .. }] = &node.settings()[..] else { panic!() };
        assert_eq!(detection.crop, crop, "for Write LTX");
        assert_eq!(detection.pixels_per_metre(), Some(250.0));
        assert!(detection.timing_led.enabled, "there'll be TTLs");
        assert_eq!(detection.timing_led.radius, 0.0, "and they've no spot");
        assert!(detection.custom.is_empty(), "and not twice");
    }

    #[test]
    fn a_position_has_one_led_and_what_a_pass_cant_take_waits_for_the_next() {
        let node = VrComputer::default();
        let (mut positions, mut ttls, mut logs) = buffers(&node);
        let room = positions.samples.capacity();
        let mut inbox = Inbox::default();
        for i in 0..room + 1 {
            assert!(inbox.receive(Message::Pos { time: 100.0 + i as f64, x: i as f32, y: 2.0 }));
        }
        inbox.take(&mut positions, &mut ttls, &mut logs);
        assert_eq!(positions.samples.len(), room);
        let first = positions.samples[0];
        assert_eq!(first.leds, [Some(Blob { x: 0.0, y: 2.0, area: AREA }), None]);
        assert_eq!(first.time, 100.0, "by the VR computer's clock");
        positions.samples.clear();
        inbox.take(&mut positions, &mut ttls, &mut logs);
        assert_eq!((positions.samples.len(), positions.samples[0].frame), (1, room as u64), "in order, numbered");
    }

    /// What comes after new settings is by them (a TTL on a channel only they have, say): it waits for the passes that
    /// have them, rather than be judged by these, unless it's recording (when they wait) or they're a while coming.
    #[test]
    fn what_comes_after_new_settings_waits_for_them() {
        let mut inbox = Inbox::default();
        let task = |name: &str| Custom::from([("task".to_string(), name.to_string())]);
        assert!(!inbox.waiting(&task("first"), false), "nothing new");
        inbox.receive(parse(r#"{"type": "settings", "settings": {"task": "second"}}"#));
        assert!(inbox.waiting(&task("first"), false), "on their way");
        assert!(!inbox.waiting(&task("first"), true), "recording, when they wait for it to stop");
        assert!(!inbox.waiting(&task("second"), false), "they're here");
        assert_eq!(inbox.settings_to_update(&task("first"), false), Some(task("second")));
        assert!(inbox.waiting(&task("first"), false), "triggered, till they're applied");
        inbox.triggered = Some(Instant::now() - WAIT_FOR_SETTINGS);
        assert!(!inbox.waiting(&task("first"), false), "for a while, at most");
    }

    #[test]
    fn settings_sent_while_recording_wait_for_it_to_stop() {
        let mut inbox = Inbox::default();
        let send = |inbox: &mut Inbox, task: &str| {
            let line = format!(r#"{{"type": "settings", "settings": {{"task": "{task}"}}}}"#);
            inbox.receive(parse(&line));
        };
        let task = |name: &str| Custom::from([("task".to_string(), name.to_string())]);
        send(&mut inbox, "first");
        assert_eq!(inbox.settings_to_update(&Custom::new(), false), Some(task("first")));
        assert_eq!(inbox.settings_to_update(&Custom::new(), false), None, "giving the GUI a moment");
        assert_eq!(inbox.settings_to_update(&task("first"), false), None, "it has them");
        assert_eq!(inbox.settings, None);

        // The same again changes nothing, recording or not: it's what's sent on connecting again, say.
        send(&mut inbox, "first");
        assert_eq!(inbox.settings_to_update(&task("first"), true), None);
        assert!(!inbox.held && inbox.settings.is_none());

        // The latest of those sent while recording is kept, however many passes go by.
        send(&mut inbox, "second");
        send(&mut inbox, "third");
        assert_eq!(inbox.settings_to_update(&task("first"), true), None);
        assert!(inbox.held);
        assert_eq!(inbox.settings_to_update(&task("first"), true), None);
        assert_eq!(inbox.settings_to_update(&task("first"), false), Some(task("third")));

        // If they don't get through, as recording starts, they're still there to propose.
        inbox.triggered = Some(Instant::now() - TRIGGER_AGAIN);
        assert_eq!(inbox.settings_to_update(&task("first"), false), Some(task("third")));
    }

    #[test]
    fn too_many_messages_are_counted_not_kept() {
        let mut inbox = Inbox::default();
        let idx = NonZeroU8::MIN;
        assert!((0..QUEUE).all(|_| inbox.receive(Message::Ttl { time: 0.0, idx, value: 1 })));
        assert!(!inbox.receive(Message::Ttl { time: 0.0, idx, value: 0 }));
        assert_eq!(inbox.ttls.len(), QUEUE);
    }

    #[test]
    fn ttls_are_on_their_channel_from_1_and_those_beyond_the_settings_are_left_out() {
        let mut inbox = Inbox::default();
        for (idx, value) in [(2, 1), (1, 1), (3, 0), (2, 0)] {
            inbox.receive(Message::Ttl { time: 0.0, idx: NonZeroU8::new(idx).unwrap(), value });
        }
        let mut events: Vec<Event> = inbox.ttls.drain(..).collect();
        let lines = |events: &[Event]| events.iter().map(|e| (e.line, e.high)).collect::<Vec<_>>();
        assert_eq!(lines(&events), [(1, true), (0, true), (2, false), (1, false)], "line 0 is idx 1");
        assert_eq!(keep_channels(&mut events, 2), Some(3), "said once");
        assert_eq!(lines(&events), [(1, true), (0, true), (1, false)]);
        assert_eq!(keep_channels(&mut events, 2), None);
    }

    #[test]
    fn its_state_says_how_long_its_been_acquiring_and_recording() {
        let state = |clock: Clock| clock.message().map(|m| serde_json::from_str::<Value>(&m).unwrap());
        assert_eq!(state(Clock::default()), None, "not before the first pass");
        let acquiring = Clock { acquiring: Some(12.3400000001), recording: None };
        let expected = serde_json::json!({"type": "state", "acquiring_for": 12.34, "recording_for": null});
        assert_eq!(state(acquiring), Some(expected));
        let recording = Clock { recording: Some(2.34), ..acquiring };
        assert_eq!(state(recording).unwrap()["recording_for"], 2.34);
        assert!(acquiring.message().unwrap().ends_with("}\n"), "a line");
    }

    #[test]
    fn it_reads_messages_over_tcp_and_connects_again() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let connection = Connection::start(("127.0.0.1".into(), port));
        let shared = connection.running.shared.clone();
        let wait = |done: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done() {
                assert!(Instant::now() < deadline, "timed out");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let (mut stream, _) = listener.accept().unwrap();
        // Its state, once there's been a pass, and as soon as recording starts.
        *shared.clock.lock().unwrap() = Clock { acquiring: Some(1.5), recording: None };
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut states = BufReader::new(stream.try_clone().unwrap());
        let mut state = || {
            let mut line = String::new();
            states.read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        };
        assert_eq!(state()["acquiring_for"], 1.5);
        *shared.clock.lock().unwrap() = Clock { acquiring: Some(1.6), recording: Some(0.1) };
        let started = Instant::now();
        assert_eq!(state()["recording_for"], 0.1);
        assert!(started.elapsed() < STATE_EVERY / 2, "not waiting a second");
        stream.write_all(b"{\"type\": \"settings\", \"settings\": {\"task\": \"no window\"}}\n").unwrap();
        stream
            .write_all(
                b"{\"type\": \"ttl\", \"time\": 7, \"idx\": 1, \"value\": 1}\nnot json\n{\"type\": \"pos\", \"time\": 8, \"x\"",
            )
            .unwrap();
        stream.flush().unwrap();
        wait(&|| shared.status.lock().unwrap().not_understood == 1);
        assert_eq!(shared.status.lock().unwrap().refused, 1);
        assert_eq!(shared.inbox.lock().unwrap().settings, None, "refused");
        // A message in two halves, either side of a read timing out.
        std::thread::sleep(READ_TIMEOUT * 2);
        stream.write_all(b": 3, \"y\": 4}\n").unwrap();
        wait(&|| shared.inbox.lock().unwrap().positions.len() == 1);
        assert_eq!(shared.status.lock().unwrap().messages, 2);
        let ttl = shared.inbox.lock().unwrap().ttls[0];
        assert!(ttl.high && ttl.time == Some(7.0));
        // Its clone too, or the connection stays open.
        drop((stream, states));
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"{\"type\": \"log\", \"time\": 9, \"line\": \"back\"}\n").unwrap();
        wait(&|| shared.inbox.lock().unwrap().logs.len() == 1);
        // A connection carries on for another epoch's use of it.
        let another = Connection::start(("127.0.0.1".into(), port));
        assert!(Arc::ptr_eq(&another.running, &connection.running));
    }
}
