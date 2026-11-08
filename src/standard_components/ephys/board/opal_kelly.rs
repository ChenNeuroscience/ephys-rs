//! Boards on Opal Kelly FPGAs (XEM6010 over USB 2, XEM6310 over USB 3) running Intan's Rhythm firmware: the
//! Open Ephys Acquisition Board before generation 3, and Intan's RHD USB Interface Board. Ported from the
//! Open Ephys acquisition board plugins (`Rhd2000EvalBoard`, `AcqBoardOpalKelly`).
//!
//! Opal Kelly's FrontPanel library is loaded at runtime, so building needs nothing from Opal Kelly. The FPGA is
//! configured from a bitfile (`rhd2000.bit`, `rhd2000_usb3.bit`, `intan_rhd_usb.bit`). The library and the Open Ephys
//! bitfiles come with the program, from the Open Ephys acquisition board plugin's release (see scripts/package.py),
//! and are otherwise looked for where the Open Ephys GUI puts them; where they're looked for first is a global
//! setting (`OpalKellySettings`).
//!
//! This is the transport: the rest is in `rhythm`, shared with the generation 3 board. Not yet tried on hardware.

use std::ffi::{CString, c_char, c_int, c_long, c_void};
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use eframe::egui;
use libloading::Library;
use serde::{Deserialize, Serialize};

use super::rhythm::{RhythmBoard, Transport, sample_bytes};
use crate::core::global_settings::GlobalSettings;
use crate::core::search_folders::{self, SearchFolders};
use crate::core::theme::Colours;

type Handle = *mut c_void;

/// Which board: they differ in their bitfile, digital inputs and analogue input range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpalKellyKind {
    /// The Open Ephys Acquisition Board, generations 1 and 2.
    #[default]
    OpenEphys,
    /// Intan's RHD USB Interface Board.
    IntanRhdUsb,
}

impl OpalKellyKind {
    pub fn name(self) -> &'static str {
        match self {
            OpalKellyKind::OpenEphys => "Open Ephys Acquisition Board (gen 1/2)",
            OpalKellyKind::IntanRhdUsb => "Intan RHD USB Interface Board",
        }
    }

    pub fn ttl_lines(self) -> u32 {
        match self {
            OpalKellyKind::OpenEphys => 8,
            OpalKellyKind::IntanRhdUsb => 16,
        }
    }

    fn bitfile(self, usb3: bool) -> &'static str {
        match (self, usb3) {
            (OpalKellyKind::OpenEphys, false) => "rhd2000.bit",
            (OpalKellyKind::OpenEphys, true) => "rhd2000_usb3.bit",
            (OpalKellyKind::IntanRhdUsb, _) => "intan_rhd_usb.bit",
        }
    }

    /// Volts from an analogue input's reading.
    fn adc_volts(self, raw: u16) -> f32 {
        match self {
            // ±5 V, and the board's offset.
            OpalKellyKind::OpenEphys => 0.000_152_587_89 * raw as f32 - 5.0 - 0.4096,
            // 0 to 3.3 V.
            OpalKellyKind::IntanRhdUsb => 0.000_050_354 * raw as f32,
        }
    }
}

#[cfg(windows)]
const LIBRARY_NAMES: &[&str] = &["okFrontPanel.dll"];
#[cfg(target_os = "macos")]
const LIBRARY_NAMES: &[&str] = &["libokFrontPanel.1.dylib", "libokFrontPanel.dylib"];
#[cfg(all(unix, not(target_os = "macos")))]
const LIBRARY_NAMES: &[&str] = &["libokFrontPanel.so"];

const XEM6010LX45: c_int = 13;
const XEM6310LX45: c_int = 21;
const BOARD_ID_USB2: u32 = 500;
const BOARD_ID_USB3: u32 = 600;
const USB3_BLOCK_SIZE: c_int = 1024;
const DDR_BLOCK_SIZE: u32 = 32;
/// Samples in each read from the board.
const SAMPLES_PER_BLOCK_USB2: usize = 300;
const SAMPLES_PER_BLOCK_USB3: usize = 256;
/// How long a read waits for data before giving up, in milliseconds: FrontPanel's own is 10 seconds, which a read of
/// more than the board will ever send (the end of the data, once it's stopped) would wait out. Well over the slowest
/// read while it's running: a block, at the slowest sample rate, 1 kHz.
const READ_TIMEOUT_MS: c_int = 1000;

// Endpoints.
const WIRE_IN_RESET_RUN: c_int = 0x00;
const WIRE_IN_MAX_TIME_STEP_LSB: c_int = 0x01;
const WIRE_IN_MAX_TIME_STEP_MSB: c_int = 0x02;
const WIRE_IN_DATA_FREQ_PLL: c_int = 0x03;
const WIRE_IN_MISO_DELAY: c_int = 0x04;
const WIRE_IN_CMD_RAM_ADDR: c_int = 0x05;
const WIRE_IN_CMD_RAM_BANK: c_int = 0x06;
const WIRE_IN_CMD_RAM_DATA: c_int = 0x07;
const WIRE_IN_AUX_CMD_BANK: [c_int; 3] = [0x08, 0x09, 0x0a];
const WIRE_IN_AUX_CMD_LENGTH: [c_int; 3] = [0x0b, 0x0c, 0x0d];
const WIRE_IN_AUX_CMD_LOOP: [c_int; 3] = [0x0e, 0x0f, 0x10];
const WIRE_IN_LED_DISPLAY: c_int = 0x11;
const WIRE_IN_DATA_STREAM_SEL_1234: c_int = 0x12;
const WIRE_IN_DATA_STREAM_SEL_5678: c_int = 0x13;
const WIRE_IN_DATA_STREAM_EN: c_int = 0x14;
const WIRE_IN_TTL_OUT: c_int = 0x15;
const WIRE_IN_DAC_SOURCE: [c_int; 8] = [0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d];
const WIRE_IN_DAC_MANUAL: c_int = 0x1e;
const WIRE_IN_MULTI_USE: c_int = 0x1f;
const TRIG_IN_DCM_PROG: c_int = 0x40;
const TRIG_IN_SPI_START: c_int = 0x41;
const TRIG_IN_RAM_WRITE: c_int = 0x42;
const TRIG_IN_DAC_THRESH: c_int = 0x43;
const TRIG_IN_EXT_FAST_SETTLE: c_int = 0x45;
const TRIG_IN_EXT_DIG_OUT: c_int = 0x46;
const TRIG_IN_OPEN_EPHYS: c_int = 0x5a;
const WIRE_OUT_NUM_WORDS_LSB: c_int = 0x20;
const WIRE_OUT_NUM_WORDS_MSB: c_int = 0x21;
const WIRE_OUT_SPI_RUNNING: c_int = 0x22;
const WIRE_OUT_DATA_CLK_LOCKED: c_int = 0x24;
const WIRE_OUT_BOARD_ID: c_int = 0x3e;
const PIPE_OUT_DATA: c_int = 0xa0;

macro_rules! frontpanel_api {
    ($($name:ident($($arg:ty),*) -> $ret:ty;)*) => {
        #[allow(non_snake_case)]
        struct Api {
            $($name: unsafe extern "system" fn($($arg),*) -> $ret,)*
            _library: Library,
        }

        impl Api {
            /// # Safety
            /// `library` must be Opal Kelly's FrontPanel library, whose functions have these signatures.
            unsafe fn from_library(library: Library) -> Result<Api, String> {
                unsafe {
                    Ok(Api {
                        $($name: *library
                            .get::<unsafe extern "system" fn($($arg),*) -> $ret>(stringify!($name))
                            .map_err(|e| format!("FrontPanel library is missing {}: {e}", stringify!($name)))?,)*
                        _library: library,
                    })
                }
            }
        }
    };
}

frontpanel_api! {
    okFrontPanel_Construct() -> Handle;
    okFrontPanel_Destruct(Handle) -> ();
    okFrontPanel_GetDeviceCount(Handle) -> c_int;
    okFrontPanel_GetDeviceListModel(Handle, c_int) -> c_int;
    okFrontPanel_GetDeviceListSerial(Handle, c_int, *mut c_char) -> ();
    okFrontPanel_OpenBySerial(Handle, *const c_char) -> c_int;
    okFrontPanel_IsOpen(Handle) -> c_int;
    okFrontPanel_LoadDefaultPLLConfiguration(Handle) -> c_int;
    okFrontPanel_ConfigureFPGA(Handle, *const c_char) -> c_int;
    okFrontPanel_IsFrontPanelEnabled(Handle) -> c_int;
    // Wire values are `UINT32` in FrontPanel's header (`unsigned long` in old ones, the same width on Windows).
    okFrontPanel_SetWireInValue(Handle, c_int, u32, u32) -> c_int;
    okFrontPanel_UpdateWireIns(Handle) -> c_int;
    okFrontPanel_UpdateWireOuts(Handle) -> c_int;
    okFrontPanel_GetWireOutValue(Handle, c_int) -> u32;
    okFrontPanel_ActivateTriggerIn(Handle, c_int, c_int) -> c_int;
    okFrontPanel_ReadFromPipeOut(Handle, c_int, c_long, *mut u8) -> c_long;
    okFrontPanel_ReadFromBlockPipeOut(Handle, c_int, c_int, c_long, *mut u8) -> c_long;
    okFrontPanel_ResetFPGA(Handle) -> c_int;
    okFrontPanel_Close(Handle) -> ();
}

/// Where the Opal Kelly boards' library and bitfiles are looked for: a tab in Settings….
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OpalKellySettings {
    /// Looked in first.
    pub folders: SearchFolders,
}

impl OpalKellySettings {
    /// Where FrontPanel's looked for: the folders chosen, with the program, where the Open Ephys GUI puts it, then the
    /// system's search path.
    fn library_candidates(&self) -> Vec<PathBuf> {
        let mut candidates = self.folders.candidates(LIBRARY_NAMES, &default_dirs());
        candidates.extend(LIBRARY_NAMES.iter().map(PathBuf::from));
        candidates
    }

    fn bitfile(&self, name: &str) -> Option<PathBuf> {
        self.folders.find(&[name], &default_dirs())
    }
}

impl GlobalSettings for OpalKellySettings {
    const NAME: &'static str = "Open Ephys gen 1/2";

    fn ui(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        search_folders::note(
            ui,
            colours,
            "Where to look first for Opal Kelly's FrontPanel library and the bitfiles, for the Open Ephys \
             Acquisition Board (generations 1 and 2) and Intan's RHD USB Interface Board. The library and the Open \
             Ephys bitfiles come with ephys-rs, and are looked for where the Open Ephys GUI puts them too.",
        );
        self.folders.ui(ui, colours);
        ui.add_space(8.0);
        let library = self.library_candidates().into_iter().find(|p| p.is_file());
        let mut found = vec![("FrontPanel library", library)];
        for name in ["rhd2000.bit", "rhd2000_usb3.bit", "intan_rhd_usb.bit"] {
            found.push((name, self.bitfile(name)));
        }
        search_folders::found_ui(ui, colours, &found);
        ui.add_space(4.0);
        search_folders::note(ui, colours, search_folders::LOADED_ONCE);
        ui.add_space(8.0);
        search_folders::get_them_ui(
            ui,
            colours,
            &[
                ("The Open Ephys GUI, with its Acquisition Board plugin", "https://open-ephys.org/gui"),
                ("Open Ephys's guide to the Acquisition Board", "https://open-ephys.github.io/acq-board-docs/"),
                ("Opal Kelly's FrontPanel, and its USB driver", "https://opalkelly.com/products/frontpanel/"),
                ("Intan's software, with its board's bitfile", "https://intantech.com/downloads.html"),
            ],
        );
    }
}

/// Where the boards' libraries and bitfiles are, unless they're in folders chosen: with this program, then where
/// the Open Ephys GUI keeps what its plugins come with.
pub(super) fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = search_folders::with_program();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let saved_state: Vec<PathBuf> = if cfg!(windows) {
        ["ProgramData", "APPDATA", "LOCALAPPDATA"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|d| PathBuf::from(d).join("Open Ephys"))
            .collect()
    } else if cfg!(target_os = "macos") {
        home.map(|h| vec![h.join("Library/Application Support/open-ephys")]).unwrap_or_default()
    } else {
        home.map(|h| vec![h.join(".config/open-ephys")]).unwrap_or_default()
    };
    for dir in saved_state {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut shared: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("shared")))
            .collect();
        shared.sort();
        shared.reverse(); // the newest plugin API first
        dirs.extend(shared);
    }
    dirs
}

impl Api {
    /// Loads the library once it's found (it's never unloaded); until then, tries again each time.
    fn get(settings: &OpalKellySettings) -> io::Result<&'static Api> {
        static API: OnceLock<Api> = OnceLock::new();
        if let Some(api) = API.get() {
            return Ok(api);
        }
        let candidates = settings.library_candidates();
        let mut errors = Vec::new();
        for candidate in &candidates {
            // Safety: loading a library runs its initialisers; FrontPanel's are benign.
            match unsafe { Library::new(candidate) } {
                Ok(library) => {
                    // Safety: it's FrontPanel, by name.
                    let api = unsafe { Api::from_library(library) }.map_err(io::Error::other)?;
                    return Ok(API.get_or_init(|| api));
                }
                Err(e) => errors.push(format!("{}: {e}", candidate.display())),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Opal Kelly's FrontPanel library not found. Tried {}", errors.join("; ")),
        ))
    }
}

/// An open Opal Kelly device.
struct FrontPanel {
    api: &'static Api,
    handle: Handle,
}

impl FrontPanel {
    fn set(&self, endpoint: c_int, value: u32, mask: u32) {
        unsafe { (self.api.okFrontPanel_SetWireInValue)(self.handle, endpoint, value, mask) };
    }

    fn update_ins(&self) {
        unsafe { (self.api.okFrontPanel_UpdateWireIns)(self.handle) };
    }

    /// Sets a wire in and sends it.
    fn wire(&self, endpoint: c_int, value: u32, mask: u32) {
        self.set(endpoint, value, mask);
        self.update_ins();
    }

    fn trigger(&self, endpoint: c_int, bit: c_int) {
        unsafe { (self.api.okFrontPanel_ActivateTriggerIn)(self.handle, endpoint, bit) };
    }

    fn update_outs(&self) {
        unsafe { (self.api.okFrontPanel_UpdateWireOuts)(self.handle) };
    }

    fn out(&self, endpoint: c_int) -> u32 {
        unsafe { (self.api.okFrontPanel_GetWireOutValue)(self.handle, endpoint) }
    }
}

impl FrontPanel {
    /// Has reads give up after `READ_TIMEOUT_MS`, if the library can (it's been able to for years, but isn't needed for
    /// anything else, so an old one without it isn't refused).
    fn set_read_timeout(&self) {
        // Safety: FrontPanel's `okFrontPanel_SetTimeout(okFrontPanel_HANDLE, int)`, in milliseconds.
        let set =
            unsafe { self.api._library.get::<unsafe extern "system" fn(Handle, c_int)>(b"okFrontPanel_SetTimeout") };
        match set {
            Ok(set) => unsafe { set(self.handle, READ_TIMEOUT_MS) },
            Err(_) => log::debug!("FrontPanel can't set a timeout for reads: stopping the board may take a while"),
        }
    }
}

impl Drop for FrontPanel {
    fn drop(&mut self) {
        unsafe {
            (self.api.okFrontPanel_Close)(self.handle);
            (self.api.okFrontPanel_Destruct)(self.handle);
        }
    }
}

/// Talks to a Rhythm board on an Opal Kelly FPGA.
pub struct OpalKelly {
    fp: FrontPanel,
    kind: OpalKellyKind,
    usb3: bool,
    enabled_streams: u32,
    running: bool,
}

// The handle is only used from the thread that owns the board.
unsafe impl Send for OpalKelly {}

/// Opens the first Rhythm board found and configures its FPGA, finding the library and bitfile as `settings` say.
pub fn open(kind: OpalKellyKind, settings: &OpalKellySettings) -> io::Result<RhythmBoard<OpalKelly>> {
    let api = Api::get(settings)?;
    let handle = unsafe { (api.okFrontPanel_Construct)() };
    if handle.is_null() {
        return Err(io::Error::other("FrontPanel couldn't start"));
    }
    let fp = FrontPanel { api, handle };
    let count = unsafe { (api.okFrontPanel_GetDeviceCount)(handle) };
    let mut usb3 = None;
    let mut seen = Vec::new();
    for i in 0..count {
        let model = unsafe { (api.okFrontPanel_GetDeviceListModel)(handle, i) };
        let mut serial = [0 as c_char; 64];
        unsafe { (api.okFrontPanel_GetDeviceListSerial)(handle, i, serial.as_mut_ptr()) };
        let serial_text = unsafe { std::ffi::CStr::from_ptr(serial.as_ptr()) }.to_string_lossy().into_owned();
        seen.push(format!("model {model} ({serial_text})"));
        if (model == XEM6010LX45 || model == XEM6310LX45)
            && unsafe { (api.okFrontPanel_OpenBySerial)(handle, serial.as_ptr()) } == 0
        {
            usb3 = Some(model == XEM6310LX45);
            break;
        }
    }
    let Some(usb3) = usb3.filter(|_| unsafe { (api.okFrontPanel_IsOpen)(handle) } != 0) else {
        let found = if seen.is_empty() { "none".into() } else { seen.join(", ") };
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no Rhythm board (an Opal Kelly XEM6010 or XEM6310) found; Opal Kelly devices: {found}"),
        ));
    };
    fp.set_read_timeout();
    unsafe { (api.okFrontPanel_LoadDefaultPLLConfiguration)(handle) };
    let mut board = OpalKelly { fp, kind, usb3, enabled_streams: 0, running: false };
    board.configure_fpga(settings)?;
    board.initialize();
    RhythmBoard::new(board)
}

impl OpalKelly {
    fn configure_fpga(&mut self, settings: &OpalKellySettings) -> io::Result<()> {
        let name = self.kind.bitfile(self.usb3);
        let Some(path) = settings.bitfile(name) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("bitfile {name} not found: add the folder it's in, in Settings… › {}", OpalKellySettings::NAME),
            ));
        };
        let c_path = CString::new(path.to_string_lossy().as_bytes()).map_err(io::Error::other)?;
        let api = self.fp.api;
        let error = unsafe { (api.okFrontPanel_ConfigureFPGA)(self.fp.handle, c_path.as_ptr()) };
        if error != 0 {
            return Err(io::Error::other(format!(
                "configuring the FPGA from {} failed (error {error})",
                path.display()
            )));
        }
        if unsafe { (api.okFrontPanel_IsFrontPanelEnabled)(self.fp.handle) } == 0 {
            return Err(io::Error::other("the FPGA doesn't support FrontPanel after configuring"));
        }
        self.fp.update_outs();
        let id = self.fp.out(WIRE_OUT_BOARD_ID);
        let expected = if self.usb3 { BOARD_ID_USB3 } else { BOARD_ID_USB2 };
        if id != expected {
            return Err(io::Error::other(format!(
                "{} isn't Rhythm firmware for this board (board ID {id})",
                path.display()
            )));
        }
        Ok(())
    }

    /// The firmware's settings that aren't the board's business (DACs, digital outputs...), as Intan's
    /// `initialize` leaves them; the rest is `RhythmBoard`'s.
    fn initialize(&mut self) {
        self.reset();
        self.fp.wire(WIRE_IN_RESET_RUN, 0, 0x04); // no DSP settle
        self.fp.wire(WIRE_IN_TTL_OUT, 0, 0xffff);
        let (dac_enable, dac_stream) = if self.usb3 { (0x0400, 0x03e0) } else { (0x0200, 0x01e0) };
        for endpoint in WIRE_IN_DAC_SOURCE {
            self.fp.wire(endpoint, 0, dac_enable | dac_stream | 0x001f);
        }
        self.fp.wire(WIRE_IN_DAC_MANUAL, 32768, 0xffff);
        self.fp.wire(WIRE_IN_RESET_RUN, 0, 0xe000); // DAC gain
        self.fp.wire(WIRE_IN_RESET_RUN, 0, 0x1fc0); // audio noise suppression
        self.fp.wire(WIRE_IN_RESET_RUN, 1 << 3, 0x0008); // digital outputs 0-7 are DAC comparators
        for dac in 0..8 {
            self.multi_use(32768, TRIG_IN_DAC_THRESH, dac);
            self.multi_use(1, TRIG_IN_DAC_THRESH, dac + 8);
        }
        self.multi_use(0, TRIG_IN_EXT_FAST_SETTLE, 0);
        self.multi_use(0, TRIG_IN_EXT_FAST_SETTLE, 1);
        for port in 0..4 {
            self.multi_use(0, TRIG_IN_EXT_DIG_OUT, port);
            self.multi_use(0, TRIG_IN_EXT_DIG_OUT, port + 4);
        }
        if self.kind == OpalKellyKind::OpenEphys {
            self.multi_use(1, TRIG_IN_OPEN_EPHYS, 0); // board LEDs on
        }
    }

    /// Sets the multi-use wire and fires a trigger, as many of the firmware's settings are made.
    fn multi_use(&self, value: u32, trigger: c_int, bit: c_int) {
        self.fp.wire(WIRE_IN_MULTI_USE, value, 0xffff_ffff);
        self.fp.trigger(trigger, bit);
    }

    fn reset(&self) {
        self.fp.wire(WIRE_IN_RESET_RUN, 0x01, 0x01);
        self.fp.wire(WIRE_IN_RESET_RUN, 0x00, 0x01);
        if self.usb3 {
            self.multi_use(USB3_BLOCK_SIZE as u32 / 4, TRIG_IN_OPEN_EPHYS, 16);
            self.multi_use(DDR_BLOCK_SIZE, TRIG_IN_OPEN_EPHYS, 17);
        }
    }

    fn run(&self) {
        self.fp.update_outs();
        self.fp.trigger(TRIG_IN_SPI_START, 0);
    }

    fn is_running(&self) -> bool {
        self.fp.update_outs();
        self.fp.out(WIRE_OUT_SPI_RUNNING) & 1 != 0
    }

    fn words_in_fifo(&self) -> u32 {
        self.fp.update_outs();
        (self.fp.out(WIRE_OUT_NUM_WORDS_MSB) << 16) + self.fp.out(WIRE_OUT_NUM_WORDS_LSB)
    }

    /// Waits for `done`, giving up after a couple of seconds.
    fn wait(&self, what: &str, done: impl Fn(&Self) -> bool) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !done(self) {
            if Instant::now() > deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, format!("the board didn't {what}")));
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        Ok(())
    }

    /// Reads `bytes` from the board into `buf`.
    fn read_bytes(&self, bytes: usize, buf: &mut Vec<u8>) -> io::Result<()> {
        buf.resize(bytes, 0);
        let (api, handle) = (self.fp.api, self.fp.handle);
        let got = unsafe {
            if self.usb3 {
                (api.okFrontPanel_ReadFromBlockPipeOut)(
                    handle,
                    PIPE_OUT_DATA,
                    USB3_BLOCK_SIZE,
                    bytes as c_long,
                    buf.as_mut_ptr(),
                )
            } else {
                (api.okFrontPanel_ReadFromPipeOut)(handle, PIPE_OUT_DATA, bytes as c_long, buf.as_mut_ptr())
            }
        };
        if got < 0 {
            return Err(io::Error::other(format!("reading from the board failed (error {got})")));
        }
        Ok(())
    }

    /// Empties what the board has waiting. Over USB 3, reads are of whole blocks: a part block is read as a whole one,
    /// waiting for the rest, unless `whole_blocks` (once it's stopped, when the rest won't come: what's left of a block
    /// is cleared when it's reset, closing it).
    fn flush(&self, whole_blocks: bool) {
        let limit = Instant::now() + Duration::from_secs(2);
        let mut buf = Vec::new();
        loop {
            let words = self.words_in_fifo() as usize;
            if words == 0 || Instant::now() > limit {
                break;
            }
            let bytes = (2 * words).min(1 << 20);
            let block = USB3_BLOCK_SIZE as usize;
            let bytes = match (self.usb3, whole_blocks) {
                (false, _) => bytes,
                (true, false) => bytes.div_ceil(block) * block,
                (true, true) => bytes / block * block,
            };
            if bytes == 0 {
                break;
            }
            if self.read_bytes(bytes, &mut buf).is_err() {
                break;
            }
        }
    }

    fn samples_per_block(&self) -> usize {
        if self.usb3 { SAMPLES_PER_BLOCK_USB3 } else { SAMPLES_PER_BLOCK_USB2 }
    }
}

impl Transport for OpalKelly {
    fn description(&self) -> String {
        format!("{} ({})", self.kind.name(), if self.usb3 { "USB 3" } else { "USB 2" })
    }

    fn max_streams(&self) -> u32 {
        if self.usb3 { 16 } else { 8 }
    }

    fn ttl_lines(&self) -> u32 {
        self.kind.ttl_lines()
    }

    fn adc_volts(&self, raw: u16) -> f32 {
        self.kind.adc_volts(raw)
    }

    fn set_sample_rate(&mut self, rate: u32) -> io::Result<()> {
        // The data clock is 2800 × the sample rate, from a PLL multiplying the board's 100 MHz by M / D.
        let (m, d) = match rate {
            1000 => (7, 125),
            1250 => (7, 100),
            1500 => (21, 250),
            2000 => (14, 125),
            2500 => (35, 250),
            3000 => (21, 125),
            3333 => (14, 75),
            4000 => (28, 125),
            5000 => (7, 25),
            6250 => (7, 20),
            8000 => (112, 250),
            10000 => (14, 25),
            12500 => (7, 10),
            15000 => (21, 25),
            20000 => (28, 25),
            25000 => (35, 25),
            30000 => (42, 25),
            _ => return Err(io::Error::other(format!("{rate} Hz isn't one of the board's sample rates"))),
        };
        self.wait("finish programming its clock", |b| {
            b.fp.update_outs();
            b.fp.out(WIRE_OUT_DATA_CLK_LOCKED) & 0x0002 != 0
        })?;
        self.fp.wire(WIRE_IN_DATA_FREQ_PLL, 256 * m + d, 0xffff_ffff);
        self.fp.trigger(TRIG_IN_DCM_PROG, 0);
        self.wait("lock its data clock", |b| {
            b.fp.update_outs();
            b.fp.out(WIRE_OUT_DATA_CLK_LOCKED) & 0x0001 != 0
        })
    }

    fn upload_command_list(&mut self, list: &[u32], slot: usize, bank: u32) {
        for (i, &command) in list.iter().enumerate() {
            self.fp.set(WIRE_IN_CMD_RAM_DATA, command, 0xffff_ffff);
            self.fp.set(WIRE_IN_CMD_RAM_ADDR, i as u32, 0xffff_ffff);
            self.fp.set(WIRE_IN_CMD_RAM_BANK, bank, 0xffff_ffff);
            self.fp.update_ins();
            self.fp.trigger(TRIG_IN_RAM_WRITE, slot as c_int);
        }
    }

    fn select_aux_bank(&mut self, slot: usize, bank: u32) {
        for port in 0..4 {
            self.fp.set(WIRE_IN_AUX_CMD_BANK[slot], bank << (4 * port), 0x000f << (4 * port));
        }
        self.fp.update_ins();
    }

    fn select_aux_length(&mut self, slot: usize, end: u32) {
        self.fp.set(WIRE_IN_AUX_CMD_LOOP[slot], 0, 0xffff_ffff);
        self.fp.set(WIRE_IN_AUX_CMD_LENGTH[slot], end, 0xffff_ffff);
        self.fp.update_ins();
    }

    fn set_cable_delay(&mut self, port: usize, delay: u32) {
        self.fp.wire(WIRE_IN_MISO_DELAY, delay.min(15) << (4 * port), 0x000f << (4 * port));
    }

    fn set_data_source(&mut self, stream: u32, source: u32) {
        let endpoint = if (stream % 8) < 4 { WIRE_IN_DATA_STREAM_SEL_1234 } else { WIRE_IN_DATA_STREAM_SEL_5678 };
        let shift = 4 * (stream % 4) + if stream >= 8 { 16 } else { 0 };
        self.fp.wire(endpoint, source << shift, 0x000f << shift);
    }

    fn enable_streams(&mut self, count: u32) {
        let bits = if count >= 32 { u32::MAX } else { (1 << count) - 1 };
        self.fp.wire(WIRE_IN_DATA_STREAM_EN, bits, (1 << self.max_streams()) - 1);
        self.enabled_streams = count;
    }

    fn set_continuous(&mut self, on: bool) {
        self.fp.wire(WIRE_IN_RESET_RUN, if on { 0x02 } else { 0 }, 0x02);
    }

    fn set_max_time_step(&mut self, steps: u32) {
        self.fp.set(WIRE_IN_MAX_TIME_STEP_LSB, steps & 0xffff, 0xffff_ffff);
        self.fp.set(WIRE_IN_MAX_TIME_STEP_MSB, steps >> 16, 0xffff_ffff);
        self.fp.update_ins();
    }

    fn run_once(&mut self, samples: u32, buf: &mut Vec<u8>) -> io::Result<()> {
        // Over USB 3, reads are of whole 1 kB blocks: 256 samples always make some.
        let samples = if self.usb3 { samples.div_ceil(256) * 256 } else { samples };
        self.set_max_time_step(samples);
        self.set_continuous(false);
        self.run();
        self.wait("finish running", |b| !b.is_running())?;
        self.read_bytes(samples as usize * sample_bytes(self.enabled_streams), buf)
    }

    fn start(&mut self) -> io::Result<()> {
        self.flush(false);
        self.set_continuous(true);
        self.run();
        self.running = true;
        Ok(())
    }

    fn read(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let samples = self.samples_per_block();
        let bytes = samples * sample_bytes(self.enabled_streams);
        if !self.usb3 && (self.words_in_fifo() as usize) < bytes / 2 {
            std::thread::sleep(Duration::from_millis(1));
            return Ok(0);
        }
        self.read_bytes(bytes, buf)?;
        Ok(samples)
    }

    fn stop(&mut self) {
        if self.running {
            self.set_continuous(false);
            self.set_max_time_step(0);
            self.flush(true);
            self.running = false;
        }
    }

    fn leds(&mut self, running: bool) {
        if self.kind == OpalKellyKind::OpenEphys {
            self.fp.wire(WIRE_IN_LED_DISPLAY, if running { 0b11 } else { 0b01 }, 0xff);
        }
    }
}

impl Drop for OpalKelly {
    fn drop(&mut self) {
        self.stop();
        if self.kind == OpalKellyKind::OpenEphys {
            self.fp.wire(WIRE_IN_LED_DISPLAY, 0, 0xff);
        }
        unsafe { (self.fp.api.okFrontPanel_ResetFPGA)(self.fp.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_chosen_are_looked_in_first() {
        let dir = std::env::temp_dir().join(format!("opal-kelly-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("intan_rhd_usb.bit"), b"").unwrap();
        let settings = OpalKellySettings { folders: SearchFolders(vec![dir.clone()]) };
        assert_eq!(settings.bitfile("intan_rhd_usb.bit"), Some(dir.join("intan_rhd_usb.bit")));
        assert_eq!(OpalKellySettings::default().bitfile("intan_rhd_usb.bit"), None);
        let candidates = settings.library_candidates();
        assert_eq!(candidates[0], dir.join(LIBRARY_NAMES[0]));
        assert_eq!(candidates.last(), Some(&PathBuf::from(LIBRARY_NAMES[LIBRARY_NAMES.len() - 1])));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
