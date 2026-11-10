//! Arduino: sets an Arduino's digital output pins while acquiring, while recording, or by hand, over Firmata (the
//! protocol Bonsai's Arduino nodes speak): the Arduino has to be running a Firmata sketch, StandardFirmata or one of
//! its own built on the Firmata library. It's for whatever the Arduino does with them: a sketch might start pulsing
//! a timing LED when a pin's set high, say.
//!
//! The serial port's opened on a thread of its own (see `talk`), when it starts running, and kept open from epoch to
//! epoch, as opening it resets most Arduinos. It waits for the sketch to say its Firmata version before setting any
//! pins, as what's sent while it's starting is lost; then sets each pin as the passes want, and, when it stops, sets
//! them low again.

use std::collections::BTreeSet;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use serialport::SerialPortType;

use crate::standard::pool::Pool;
use crate::standard::widgets::{combo, named_settings_grid, setting};
use crate::{DataStream, Edges};
use ephys_rs::engine::{Category, Flow, Node, NodeInfo, NodeThread, Output, Pass, PassState, ThreadContext, UiContext};
use ephys_rs::gui::widgets::{Icon, small_icon_button};

/// StandardFirmata's.
const DEFAULT_BAUD: u32 = 57600;
const BAUDS: &[u32] = &[9600, 19200, 38400, 57600, 115200];
/// The pins it can set: not 0 and 1, which are the serial port's on most Arduinos, and up to the Mega's last.
const PINS: std::ops::RangeInclusive<u8> = 2..=69;
/// USB vendors whose serial ports are taken to be Arduinos: Arduino's own, and the CH340 that clones use.
const ARDUINO_VENDORS: &[u16] = &[0x2341, 0x2a03, 0x1a86];

/// Firmata's commands (see https://github.com/firmata/protocol).
const DIGITAL_MESSAGE: u8 = 0x90;
const SET_PIN_MODE: u8 = 0xf4;
const OUTPUT: u8 = 1;
const REPORT_VERSION: u8 = 0xf9;

/// How long a read waits, and so how soon a change is sent.
const READ_TIMEOUT: Duration = Duration::from_millis(20);
/// Asking for the version, if the sketch hasn't said it by then (as one that doesn't reset when the port's opened
/// won't).
const ASK_AFTER: Duration = Duration::from_secs(5);
/// Setting the pins anyway, if there's still no version by then, warning that it may not be running Firmata.
const VERSION_WAIT: Duration = Duration::from_secs(8);
const RETRY_AFTER: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Arduino {
    /// Its serial port's name (`/dev/cu.usbmodem1101`, `COM4`): empty until it's chosen, as it's never guessed.
    pub port: String,
    pub baud: u32,
    pub pins: Vec<OutputPin>,
}

/// The Bonsai workflow's: pin 8 high while acquiring.
impl Default for Arduino {
    fn default() -> Self {
        Arduino { port: String::new(), baud: DEFAULT_BAUD, pins: vec![OutputPin::default()] }
    }
}

/// A digital pin it sets, and when it's high.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputPin {
    pub pin: u8,
    pub when: When,
    /// Whether it's high, when it's set by hand.
    pub on: bool,
}

impl Default for OutputPin {
    fn default() -> Self {
        OutputPin { pin: 8, when: When::Acquiring, on: false }
    }
}

/// When a pin's high (and otherwise it's low).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum When {
    /// From Play to Stop.
    Acquiring,
    Recording,
    /// When it's switched on in its window.
    ByHand,
}

/// Which pins are set, and which of them are high: the passes say what they want, and the thread what it's set.
#[derive(Clone, Debug, Default, PartialEq)]
struct Pins {
    outputs: BTreeSet<u8>,
    high: BTreeSet<u8>,
}

impl Arduino {
    /// The pins it sets, and which are high, by whether it's `recording`: a pin listed twice is high if either says so.
    fn wanted(&self, recording: bool) -> Pins {
        let high = |pin: &OutputPin| match pin.when {
            When::Acquiring => true,
            When::Recording => recording,
            When::ByHand => pin.on,
        };
        let outputs = self.pins.iter().map(|p| p.pin).collect();
        Pins { outputs, high: self.pins.iter().filter(|p| high(p)).map(|p| p.pin).collect() }
    }
}

impl Pins {
    /// The Firmata commands that get from what's set to `wanted`: each new pin made an output, then each port (of 8
    /// pins) whose pins change set to its pins' values. A pin no longer wanted goes low.
    fn commands_to(&mut self, wanted: &Pins) -> Vec<u8> {
        let mut commands = Vec::new();
        for &pin in wanted.outputs.difference(&self.outputs) {
            commands.extend([SET_PIN_MODE, pin, OUTPUT]);
        }
        self.outputs.extend(&wanted.outputs);
        let ports: BTreeSet<u8> = self.high.symmetric_difference(&wanted.high).map(|pin| pin / 8).collect();
        for port in ports {
            let value = wanted.high.iter().filter(|pin| *pin / 8 == port).fold(0u8, |v, pin| v | 1 << (pin % 8));
            commands.extend([DIGITAL_MESSAGE | port, value & 0x7f, value >> 7]);
        }
        self.high.clone_from(&wanted.high);
        commands
    }
}

/// The Firmata version a sketch says it speaks, if `heard` says it.
fn version(heard: &[u8]) -> Option<(u8, u8)> {
    heard.windows(3).rev().find(|w| w[0] == REPORT_VERSION && w[1] < 0x80 && w[2] < 0x80).map(|w| (w[1], w[2]))
}

/// A serial port, for choosing one.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub name: String,
    /// Its name, and what's on it, if it's USB.
    pub label: String,
    pub arduino: bool,
}

/// The serial ports there are (on a Mac, only the `/dev/cu.` of each, which is the one to use).
pub fn serial_ports() -> Vec<Found> {
    let ports = serialport::available_ports().unwrap_or_default();
    let ports = ports.into_iter().filter(|p| !p.port_name.starts_with("/dev/tty."));
    let found = |port: serialport::SerialPortInfo| match port.port_type {
        SerialPortType::UsbPort(usb) => {
            let what = usb.product.or(usb.manufacturer).unwrap_or_else(|| "USB".into());
            let label = format!("{} ({what})", port.port_name);
            Found { name: port.port_name, label, arduino: ARDUINO_VENDORS.contains(&usb.vid) }
        }
        _ => Found { label: port.port_name.clone(), name: port.port_name, arduino: false },
    };
    ports.map(found).collect()
}

/// How it's getting on with the Arduino.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// The port it's open on.
    pub open: Option<String>,
    /// Why it isn't, if it's tried.
    pub problem: Option<String>,
    /// The Firmata version the sketch said.
    pub firmata: Option<(u8, u8)>,
    /// Whether it's given up waiting for it, and sets the pins anyway.
    pub no_reply: bool,
    /// The pins it's set high.
    pub high: Vec<u8>,
}

/// The port and baud rate it's opened with.
type Key = (String, u32);

struct Shared {
    stop: AtomicBool,
    /// What the passes want.
    wanted: Mutex<Pins>,
    status: Mutex<Status>,
}

impl Shared {
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Talks to the sketch on `port` (called `name`) until told to stop, setting its pins low then, or the port's
    /// lost.
    fn talk(&self, mut port: Box<dyn serialport::SerialPort>, name: &str, baud: u32) -> Result<(), String> {
        *self.status.lock().unwrap() = Status { open: Some(name.into()), ..Default::default() };
        log::info!("Arduino: opened {name}");
        let lost = |e: std::io::Error| format!("lost {name}: {e}");
        let (opened, mut asked, mut ready) = (Instant::now(), false, false);
        let (mut heard, mut buffer, mut pins) = (Vec::new(), [0; 256], Pins::default());
        while !self.stopping() {
            match port.read(&mut buffer) {
                Ok(n) => heard.extend_from_slice(&buffer[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                Err(e) => return Err(lost(e)),
            }
            if !ready && let Some((major, minor)) = version(&heard) {
                log::info!("Arduino: its sketch speaks Firmata {major}.{minor}");
                let mut status = self.status.lock().unwrap();
                (status.firmata, status.no_reply) = (Some((major, minor)), false);
                ready = true;
            } else if !ready && !asked && opened.elapsed() >= ASK_AFTER {
                port.write_all(&[REPORT_VERSION]).map_err(lost)?;
                asked = true;
            } else if !ready && opened.elapsed() >= VERSION_WAIT {
                log::warn!(
                    "Arduino: no reply from {name}: is it running a Firmata sketch, at {baud} baud? Setting its pins \
                     anyway"
                );
                self.status.lock().unwrap().no_reply = true;
                ready = true;
            }
            // Only what's needed for a version that's split between reads.
            heard.drain(..heard.len().saturating_sub(2));
            if ready {
                let wanted = self.wanted.lock().unwrap().clone();
                let commands = pins.commands_to(&wanted);
                if !commands.is_empty() {
                    port.write_all(&commands).map_err(lost)?;
                    self.status.lock().unwrap().high = pins.high.iter().copied().collect();
                }
            }
        }
        // Stopping: every pin low, left to the system to send (not waited for, in case the Arduino's stuck). The sketch
        // may be busy, but the bytes wait for it on the Arduino.
        let low = Pins { outputs: pins.outputs.clone(), high: BTreeSet::new() };
        let commands = pins.commands_to(&low);
        if ready && !commands.is_empty() {
            port.write_all(&commands).map_err(lost)?;
        }
        Ok(())
    }
}

/// The port called `setting`. With none chosen it doesn't guess, even with just one Arduino plugged in: it could be one
/// with another job (triggering a camera, say), which opening its port would reset.
fn find(setting: &str) -> Result<String, String> {
    if setting.is_empty() {
        return Err("no port chosen: choose the Arduino's port".into());
    }
    Ok(setting.into())
}

/// The thread: opens the port, talks to the sketch until told to stop or the port's lost, and tries again.
fn run(shared: &Shared, (setting, baud): &Key) {
    while !shared.stopping() {
        let open = |name: String| {
            let port = serialport::new(&name, *baud).timeout(READ_TIMEOUT).open();
            port.map(|port| (port, name.clone())).map_err(|e| format!("couldn't open {name}: {e}"))
        };
        let Err(problem) = find(setting).and_then(open).and_then(|(port, name)| shared.talk(port, &name, *baud)) else {
            continue;
        };
        let mut status = shared.status.lock().unwrap();
        // Logged when it changes, not every time it's tried again.
        if status.problem.as_ref() != Some(&problem) {
            log::warn!("Arduino: {problem}");
        }
        *status = Status { problem: Some(problem), ..Default::default() };
        drop(status);
        let retry_at = Instant::now() + RETRY_AFTER;
        while !shared.stopping() && Instant::now() < retry_at {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// The port open on its own thread, until the last `Connection` using it lets go.
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

/// The ports open, by port and baud rate. Weak, so one closes once nothing's using it.
static RUNNING: Mutex<Vec<(Key, Weak<Running>)>> = Mutex::new(Vec::new());

/// An epoch's use of a port.
struct Connection {
    key: Key,
    running: Arc<Running>,
}

impl Connection {
    /// Uses the port, carrying on with it if it's open.
    fn start(key: Key) -> Self {
        let mut running = RUNNING.lock().unwrap();
        running.retain(|(_, connection)| connection.strong_count() > 0);
        let open = running.iter().find(|(k, _)| *k == key).and_then(|(_, connection)| connection.upgrade());
        if let Some(open) = open {
            return Connection { key, running: open };
        }
        let shared =
            Arc::new(Shared { stop: AtomicBool::new(false), wanted: Mutex::default(), status: Mutex::default() });
        let (thread_shared, thread_key) = (shared.clone(), key.clone());
        let thread = std::thread::Builder::new()
            .name("Arduino".into())
            .spawn(move || run(&thread_shared, &thread_key))
            .expect("Arduino thread");
        let connection = Arc::new(Running { shared, thread: Some(thread) });
        running.push((key.clone(), Arc::downgrade(&connection)));
        Connection { key, running: connection }
    }
}

/// While running: the port.
#[derive(Default)]
pub struct Connected {
    connection: Option<Connection>,
}

impl PassState for Connected {
    const WHY: &'static str = "The Arduino's serial port, kept open, as opening it resets the Arduino: each pass says \
                               which pins it wants high.";
}

impl Node<Edges> for Arduino {
    const INFO: NodeInfo = NodeInfo {
        name: "Arduino",
        category: Category::Other,
        description: "Sets an Arduino's pins while acquiring, while recording, or by hand, over Firmata.",
    };

    type State = Connected;
    /// How it's getting on, for its window.
    type Shared = Pool<Status>;
    type UiState = ();
    type GlobalSettings = ();

    fn outputs(&self, _inputs: &[Option<DataStream>]) -> Vec<Output<Edges>> {
        Vec::new()
    }

    fn warnings(&self, _inputs: &[Option<DataStream>]) -> Vec<String> {
        if self.pins.is_empty() { vec!["No pins to set".into()] } else { Vec::new() }
    }

    fn threads(&self) -> Vec<NodeThread> {
        vec![NodeThread {
            name: "Arduino",
            link: "pins wanted",
            flow: Flow::FromPasses,
            why: "On a thread of its own so the Arduino's serial port is waited on there, not by the passes.",
            swapped_outputs: Vec::new(),
        }]
    }

    fn thread_ui(&self, _thread: usize, ui: &mut egui::Ui, context: &ThreadContext) {
        let pool = context.shared::<Pool<Status>>();
        match pool.and_then(Pool::newest) {
            Some(status) => status_ui(ui, &status, self.baud, context.theme.warning),
            None => drop(ui.weak("Opens the Arduino's port; starts when running.")),
        }
    }

    fn initial_state(&self, _inputs: &[Option<DataStream>]) -> Connected {
        Connected { connection: Some(Connection::start((self.port.clone(), self.baud))) }
    }

    fn process(&self, pass: &mut Pass<Edges>, connected: &mut Connected) {
        let key = (self.port.clone(), self.baud);
        if connected.connection.as_ref().is_none_or(|c| c.key != key) {
            connected.connection = Some(Connection::start(key));
        }
        let Some(connection) = &connected.connection else { return };
        let shared = &connection.running.shared;
        let wanted = self.wanted(pass.recording());
        let mut pins = shared.wanted.lock().unwrap();
        if *pins != wanted {
            *pins = wanted;
        }
        drop(pins);
        let status = &shared.status;
        pass.shared::<Pool<Status>>().send_newest(pass.sequence, |shown| shown.clone_from(&status.lock().unwrap()));
    }

    fn gui_ui(&mut self, ui: &mut egui::Ui, context: &mut UiContext<Edges>) {
        let status = context.shared::<Pool<Status>>().and_then(Pool::newest).map(|s| s.clone());
        let (running, warning) = (context.running, context.theme.warning);
        ephys_rs::gui::node_window::standard(self, ui, context, |node, ui, _context| {
            ui.label("Sets the Arduino's pins over Firmata: it has to be running a Firmata sketch.");
            ui.add_space(4.0);
            named_settings_grid(ui, "arduino", |ui| {
                setting(ui, "Port", |ui| port_ui(ui, &mut node.port));
                setting(ui, "Baud rate", |ui| {
                    let bauds: Vec<(u32, String)> = BAUDS.iter().map(|b| (*b, b.to_string())).collect();
                    let options: Vec<(u32, &str)> = bauds.iter().map(|(b, s)| (*b, s.as_str())).collect();
                    combo(ui, "baud", &mut node.baud, &options).on_hover_text("StandardFirmata's is 57600")
                });
            });
            ui.add_space(4.0);
            ui.label(RichText::new("Pins").strong());
            let mut removed = None;
            egui::Grid::new("arduino pins").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
                for (i, pin) in node.pins.iter_mut().enumerate() {
                    let id = ("arduino pin", i);
                    let number =
                        ui.push_id(id, |ui| ui.add(egui::DragValue::new(&mut pin.pin).range(PINS).prefix("pin ")));
                    let options = [
                        (When::Acquiring, "high while acquiring"),
                        (When::Recording, "high while recording"),
                        (When::ByHand, "set by hand"),
                    ];
                    ui.push_id(id, |ui| combo(ui, "when", &mut pin.when, &options).labelled_by(number.inner.id));
                    if pin.when == When::ByHand {
                        ui.push_id(id, |ui| ui.checkbox(&mut pin.on, "high"));
                    } else {
                        ui.label("");
                    }
                    let remove =
                        |ui: &mut egui::Ui| small_icon_button(ui, Icon::Delete, "Remove pin", "Stop setting this pin");
                    if ui.push_id(id, remove).inner.clicked() {
                        removed = Some(i);
                    }
                    ui.end_row();
                }
            });
            if let Some(i) = removed {
                node.pins.remove(i);
            }
            if ui.button("Add a pin").clicked() {
                let unused = PINS.clone().find(|p| !node.pins.iter().any(|pin| pin.pin == *p)).unwrap_or(2);
                node.pins.push(OutputPin { pin: unused, when: When::ByHand, on: false });
            }
            ui.separator();
            match status.filter(|_| running) {
                Some(status) => status_ui(ui, &status, node.baud, warning),
                None => drop(ui.weak("Press Play to open its port.")),
            }
        });
    }
}

/// A choice of the serial ports there are.
fn port_ui(ui: &mut egui::Ui, port: &mut String) -> egui::Response {
    let selected = if port.is_empty() { "Choose…" } else { port.as_str() };
    egui::ComboBox::from_id_salt("port")
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for found in serial_ports() {
                let label = if found.arduino { format!("{} · Arduino", found.label) } else { found.label };
                if ui.selectable_label(*port == found.name, label).clicked() {
                    *port = found.name;
                }
            }
        })
        .response
}

fn status_ui(ui: &mut egui::Ui, status: &Status, baud: u32, warning: egui::Color32) {
    match (&status.problem, &status.open) {
        (Some(problem), _) => drop(ui.label(RichText::new(problem).color(warning))),
        (None, Some(open)) => match status.firmata {
            Some((major, minor)) => drop(ui.label(format!("Open on {open}: Firmata {major}.{minor}"))),
            None if status.no_reply => drop(ui.label(
                RichText::new(format!(
                    "Open on {open}, but no reply: is it running a Firmata sketch, at {baud} baud? Its pins are set \
                     anyway"
                ))
                .color(warning),
            )),
            None => drop(ui.weak(format!("Open on {open}: waiting for the Arduino to start…"))),
        },
        (None, None) => drop(ui.weak("Opening its port…")),
    }
    if status.open.is_some() && (status.firmata.is_some() || status.no_reply) {
        let high: Vec<String> = status.high.iter().map(u8::to_string).collect();
        ui.weak(if high.is_empty() { "No pins high".into() } else { format!("High: pin {}", high.join(", ")) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_no_port_chosen_it_opens_none() {
        assert_eq!(find(""), Err("no port chosen: choose the Arduino's port".into()));
        assert_eq!(find("COM4"), Ok("COM4".into()));
    }

    fn pins(outputs: &[u8], high: &[u8]) -> Pins {
        Pins { outputs: outputs.iter().copied().collect(), high: high.iter().copied().collect() }
    }

    #[test]
    fn its_pins_are_high_while_acquiring_while_recording_or_by_hand() {
        let pin = |pin, when, on| OutputPin { pin, when, on };
        let node = Arduino {
            pins: vec![
                pin(8, When::Acquiring, false),
                pin(9, When::Recording, false),
                pin(10, When::ByHand, true),
                pin(11, When::ByHand, false),
                pin(11, When::Recording, false),
            ],
            ..Default::default()
        };
        assert_eq!(node.wanted(false), pins(&[8, 9, 10, 11], &[8, 10]));
        assert_eq!(node.wanted(true), pins(&[8, 9, 10, 11], &[8, 9, 10, 11]), "either of pin 11's");
        assert_eq!(Arduino::default().wanted(false), pins(&[8], &[8]), "the Bonsai workflow's");
    }

    #[test]
    fn firmata_makes_its_pins_outputs_and_sets_each_port_they_change() {
        let mut set = Pins::default();
        assert_eq!(set.commands_to(&pins(&[8], &[8])), [0xf4, 8, 1, 0x91, 0x01, 0x00], "pin 8 is port 1's first");
        assert_eq!(set.commands_to(&pins(&[8], &[8])), [] as [u8; 0], "nothing's changed");
        assert_eq!(
            set.commands_to(&pins(&[7, 8, 13], &[7, 8, 13])),
            [
                0xf4, 7, 1, 0xf4, 13, 1, // made outputs
                0x90, 0x00, 0x01, // pin 7: port 0's eighth bit, in the second byte
                0x91, 0x21, 0x00, // pins 8 and 13
            ]
        );
        assert_eq!(set.commands_to(&pins(&[7, 8, 13], &[8])), [0x90, 0, 0, 0x91, 0x01, 0x00]);
        assert_eq!(set.commands_to(&pins(&[], &[])), [0x91, 0, 0], "those no longer wanted go low");
        assert_eq!(set.commands_to(&pins(&[8], &[8])), [0x91, 0x01, 0x00], "and are outputs already");
    }

    #[test]
    fn the_sketch_says_its_firmata_version() {
        // Firmata.begin's version, among a digital port's report and its firmware's name.
        let heard = [0x90, 0x10, 0x00, 0xf9, 2, 5, 0xf0, 0x79, 2, 5, b'a', 0, 0xf7];
        assert_eq!(version(&heard), Some((2, 5)));
        assert_eq!(version(&heard[..5]), None, "not all of it yet");
        assert_eq!(version(&[0xf9, 0x90, 0]), None, "not a version");
    }

    /// Talking to the sketch, on a pseudo-terminal standing in for the Arduino's port (which can't be opened by name, as
    /// it has no baud rate to set).
    #[cfg(unix)]
    #[test]
    fn it_waits_for_the_sketch_then_sets_its_pins_and_sets_them_low_when_it_stops() {
        use serialport::{SerialPort, TTYPort};

        let (mut arduino, mut port) = TTYPort::pair().expect("a pseudo-terminal");
        arduino.set_timeout(Duration::from_secs(10)).unwrap();
        port.set_timeout(READ_TIMEOUT).unwrap();
        let shared =
            Arc::new(Shared { stop: AtomicBool::new(false), wanted: Mutex::default(), status: Mutex::default() });
        *shared.wanted.lock().unwrap() = Arduino::default().wanted(false);
        let thread_shared = shared.clone();
        // A clone, so it's kept open: what's unread is lost when a pseudo-terminal closes, if not a real port.
        let clone = port.try_clone_native().unwrap();
        let talking = std::thread::spawn(move || thread_shared.talk(Box::new(clone), "the port", DEFAULT_BAUD));
        let status = || shared.status.lock().unwrap().clone();
        let until = |done: &dyn Fn(&Status) -> bool| {
            let start = Instant::now();
            while !done(&status()) {
                assert!(start.elapsed() < Duration::from_secs(10), "{:?}", status());
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        until(&|s| s.open.is_some());
        std::thread::sleep(Duration::from_millis(200));
        assert!(status().high.is_empty(), "nothing set before the sketch has started");
        // Split between reads.
        arduino.write_all(&[REPORT_VERSION, 2]).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        arduino.write_all(&[5]).unwrap();
        let mut sent = [0; 6];
        arduino.read_exact(&mut sent).unwrap();
        assert_eq!(sent, [0xf4, 8, 1, 0x91, 0x01, 0x00], "pin 8 high");
        until(&|s| s.firmata == Some((2, 5)) && s.high == [8]);
        *shared.wanted.lock().unwrap() = pins(&[8, 9], &[8, 9]);
        let mut sent = [0; 6];
        arduino.read_exact(&mut sent).unwrap();
        assert_eq!(sent, [0xf4, 9, 1, 0x91, 0x03, 0x00], "and pin 9");
        shared.stop.store(true, Ordering::Relaxed);
        assert_eq!(talking.join().unwrap(), Ok(()));
        let mut low = [0; 3];
        arduino.read_exact(&mut low).unwrap();
        assert_eq!(low, [0x91, 0, 0], "both low when it stops");
        drop(port);
    }

    #[test]
    fn its_port_is_kept_open_from_epoch_to_epoch_and_closed_when_its_done_with() {
        let key = ("/dev/no such port".to_string(), DEFAULT_BAUD);
        let connection = Connection::start(key.clone());
        let shared = connection.running.shared.clone();
        let again = Connection::start(key.clone());
        assert!(Arc::ptr_eq(&again.running, &connection.running), "another epoch's carries on with it");
        let other = Connection::start((key.0.clone(), 9600));
        assert!(!Arc::ptr_eq(&other.running, &connection.running), "at another baud rate it's opened again");
        let start = Instant::now();
        while shared.status.lock().unwrap().problem.is_none() {
            assert!(start.elapsed() < Duration::from_secs(10), "it tries");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(connection);
        assert!(!shared.stopping(), "still in use");
        drop(again);
        assert!(shared.stopping());
        let problem = shared.status.lock().unwrap().problem.clone().unwrap_or_default();
        assert!(problem.starts_with("couldn't open /dev/no such port"), "{problem}");
    }
}
