//! Boards on Opal Kelly FPGAs (XEM6010 over USB 2, XEM6310 over USB 3) running Intan's Rhythm firmware: the
//! Open Ephys Acquisition Board before generation 3, and Intan's RHD USB Interface Board. Ported from the
//! Open Ephys acquisition board plugins (`Rhd2000EvalBoard`, `AcqBoardOpalKelly`).
//!
//! Opal Kelly's FrontPanel library is loaded at runtime, so building needs nothing from Opal Kelly and machines
//! without it just get an error when this board is selected. The FPGA is configured from a bitfile: the Open
//! Ephys GUI's plugins come with them (`rhd2000.bit`, `rhd2000_usb3.bit`, `intan_rhd_usb.bit`), as does the
//! library, and they're looked for where the GUI puts them.
//!
//! Not yet tried on hardware.

use std::ffi::{CString, c_char, c_int, c_long, c_ulong, c_void};
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use libloading::Library;
use serde::{Deserialize, Serialize};

use super::rhd::{self, CHIP_RHD2132, CHIP_RHD2164, CHIP_RHD2216, REGISTER_59_MISO_A, RHD2132_16CH_OFFSET, Registers};
use super::{AUX_VOLTS_PER_BIT, Block, Board, BoardInfo, Config, EPHYS_MICROVOLTS_PER_BIT, Headstage, SAMPLE_RATES};

type Handle = *mut c_void;

/// Which board: they differ in their bitfile, digital inputs and analogue input range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rhythm {
    /// The Open Ephys Acquisition Board, generations 1 and 2.
    #[default]
    OpenEphys,
    /// Intan's RHD USB Interface Board.
    IntanRhdUsb,
}

impl Rhythm {
    pub fn name(self) -> &'static str {
        match self {
            Rhythm::OpenEphys => "Open Ephys Acquisition Board (gen 1/2)",
            Rhythm::IntanRhdUsb => "Intan RHD USB Interface Board",
        }
    }

    pub fn ttl_lines(self) -> u32 {
        match self {
            Rhythm::OpenEphys => 8,
            Rhythm::IntanRhdUsb => 16,
        }
    }

    fn bitfile(self, usb3: bool) -> &'static str {
        match (self, usb3) {
            (Rhythm::OpenEphys, false) => "rhd2000.bit",
            (Rhythm::OpenEphys, true) => "rhd2000_usb3.bit",
            (Rhythm::IntanRhdUsb, _) => "intan_rhd_usb.bit",
        }
    }

    /// Volts from an analogue input's reading.
    fn adc_volts(self, raw: u16) -> f32 {
        match self {
            // ±5 V, and the board's offset.
            Rhythm::OpenEphys => 0.000_152_587_89 * raw as f32 - 5.0 - 0.4096,
            // 0 to 3.3 V.
            Rhythm::IntanRhdUsb => 0.000_050_354 * raw as f32,
        }
    }
}

#[cfg(windows)]
const LIBRARY_NAME: &str = "okFrontPanel.dll";
#[cfg(target_os = "macos")]
const LIBRARY_NAME: &str = "libokFrontPanel.dylib";
#[cfg(all(unix, not(target_os = "macos")))]
const LIBRARY_NAME: &str = "libokFrontPanel.so";

const XEM6010LX45: c_int = 13;
const XEM6310LX45: c_int = 21;
const BOARD_ID_USB2: u32 = 500;
const BOARD_ID_USB3: u32 = 600;
const USB3_BLOCK_SIZE: c_int = 1024;
const DDR_BLOCK_SIZE: u32 = 32;
/// Samples in each read from the board.
const SAMPLES_PER_BLOCK_USB2: usize = 300;
const SAMPLES_PER_BLOCK_USB3: usize = 256;
/// Samples run while scanning the ports: enough for the register config command list's answers.
const SCAN_SAMPLES_USB2: u32 = 60;
const SCAN_SAMPLES_USB3: u32 = 256;
const HEADER_MAGIC: u64 = 0xc691_1999_2702_1942;
/// The data sources, in the firmware's order: each port's two MISO lines.
const SOURCES: [&str; 8] = ["A1", "A2", "B1", "B2", "C1", "C2", "D1", "D2"];
/// The second data stream of an RHD2164 (its MISO B), read with double data rate: source + 8.
const DDR: u32 = 8;

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
    okFrontPanel_SetWireInValue(Handle, c_int, c_ulong, c_ulong) -> c_int;
    okFrontPanel_UpdateWireIns(Handle) -> c_int;
    okFrontPanel_UpdateWireOuts(Handle) -> c_int;
    okFrontPanel_GetWireOutValue(Handle, c_int) -> c_ulong;
    okFrontPanel_ActivateTriggerIn(Handle, c_int, c_int) -> c_int;
    okFrontPanel_ReadFromPipeOut(Handle, c_int, c_long, *mut u8) -> c_long;
    okFrontPanel_ReadFromBlockPipeOut(Handle, c_int, c_int, c_long, *mut u8) -> c_long;
    okFrontPanel_ResetFPGA(Handle) -> c_int;
    okFrontPanel_Close(Handle) -> ();
}

/// Where the Open Ephys GUI keeps what its plugins come with, and next to this program.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        dirs.push(dir.to_path_buf());
        dirs.push(dir.join("shared"));
    }
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
    fn get(path: Option<&str>) -> io::Result<&'static Api> {
        static API: OnceLock<Api> = OnceLock::new();
        if let Some(api) = API.get() {
            return Ok(api);
        }
        let candidates: Vec<PathBuf> = match path {
            Some(p) => vec![p.into()],
            None => std::iter::once(LIBRARY_NAME.into())
                .chain(search_dirs().into_iter().map(|d| d.join(LIBRARY_NAME)))
                .collect(),
        };
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
            format!(
                "Opal Kelly's FrontPanel library not found (it comes with the Open Ephys GUI's acquisition board \
                 plugin). Tried {}",
                errors.join("; ")
            ),
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
        unsafe { (self.api.okFrontPanel_SetWireInValue)(self.handle, endpoint, value as c_ulong, mask as c_ulong) };
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
        unsafe { (self.api.okFrontPanel_GetWireOutValue)(self.handle, endpoint) as u32 }
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

/// A data stream the board reads: from which source, its chip, and which of its channels are taken.
#[derive(Clone, Copy, Debug)]
struct Stream {
    source: u32,
    /// The second stream of an RHD2164 (which has no auxiliary inputs of its own).
    second: bool,
    chip: i32,
    channels: usize,
}

pub struct OpalKellyBoard {
    fp: FrontPanel,
    kind: Rhythm,
    usb3: bool,
    info: BoardInfo,
    /// The chip found on each data source, by scanning.
    chips: [Option<i32>; 8],
    /// Each port's cable delay.
    delays: [u32; 4],
    streams: Vec<Stream>,
    enabled_streams: u32,
    registers: Registers,
    sample_rate: u32,
    /// Bytes read from the board.
    buf: Vec<u8>,
    /// Each stream's auxiliary inputs as last sampled (each is sampled every fourth sample).
    aux: Vec<[f32; 3]>,
    aux_due: Vec<[f32; 3]>,
    warnings: Vec<String>,
    running: bool,
}

// The handle is only used from the thread that owns the board.
unsafe impl Send for OpalKellyBoard {}

impl OpalKellyBoard {
    /// Opens the first Rhythm board found, configures its FPGA, and scans its ports for headstages.
    /// `bitfile` and `library` override where those are looked for.
    pub fn open(kind: Rhythm, bitfile: Option<&str>, library: Option<&str>) -> io::Result<Self> {
        let api = Api::get(library)?;
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
        unsafe { (api.okFrontPanel_LoadDefaultPLLConfiguration)(handle) };

        let mut board = OpalKellyBoard {
            fp,
            kind,
            usb3,
            info: BoardInfo::default(),
            chips: [None; 8],
            delays: [0; 4],
            streams: Vec::new(),
            enabled_streams: 0,
            registers: Registers::new(30000.0),
            sample_rate: 30000,
            buf: Vec::new(),
            aux: Vec::new(),
            aux_due: Vec::new(),
            warnings: Vec::new(),
            running: false,
        };
        board.configure_fpga(bitfile)?;
        board.initialize()?;
        board.set_sample_rate(30000)?;
        for port in 0..4 {
            board.set_cable_length(port, 0.914);
        }
        board.calibrate()?;
        board.scan()?;
        let headstages = board.found_headstages();
        board.info = BoardInfo {
            description: format!("{} ({})", kind.name(), if usb3 { "USB 3" } else { "USB 2" }),
            headstages,
            adc_channels: 8,
            ttl_lines: kind.ttl_lines(),
        };
        board.leds(&[true, false, false, false, false, false, false, false]);
        Ok(board)
    }

    fn max_streams(&self) -> u32 {
        if self.usb3 { 16 } else { 8 }
    }

    fn samples_per_block(&self) -> usize {
        if self.usb3 { SAMPLES_PER_BLOCK_USB3 } else { SAMPLES_PER_BLOCK_USB2 }
    }

    fn configure_fpga(&mut self, bitfile: Option<&str>) -> io::Result<()> {
        let name = self.kind.bitfile(self.usb3);
        let candidates: Vec<PathBuf> = match bitfile {
            Some(path) => vec![path.into()],
            None => search_dirs().into_iter().map(|d| d.join(name)).filter(|p| p.exists()).collect(),
        };
        let Some(path) = candidates.first() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "bitfile {name} not found: set where it is (it comes with the Open Ephys GUI's acquisition board plugin)"
                ),
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

    /// The firmware's state after configuring, as Intan's `initialize` leaves it.
    fn initialize(&mut self) -> io::Result<()> {
        self.reset();
        for slot in 0..3 {
            self.select_aux_bank_all(slot, 0);
            self.select_aux_length(slot, 0, 0);
        }
        self.set_continuous(true);
        self.set_max_time_step(u32::MAX);
        self.fp.wire(WIRE_IN_RESET_RUN, 0, 0x04); // no DSP settle
        // Each stream reads its port's first MISO line to begin with.
        for (stream, source) in [0, 2, 4, 6, 1, 3, 5, 7].into_iter().enumerate() {
            self.set_data_source(stream as u32, source);
            if self.usb3 {
                self.set_data_source(stream as u32 + 8, source);
            }
        }
        self.enable_streams(1);
        self.fp.wire(WIRE_IN_TTL_OUT, 0, 0xffff);
        let usb3 = self.usb3;
        let (dac_enable, dac_stream) = if usb3 { (0x0400, 0x03e0) } else { (0x0200, 0x01e0) };
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
        if self.kind == Rhythm::OpenEphys {
            self.multi_use(1, TRIG_IN_OPEN_EPHYS, 0); // board LEDs on
        }
        Ok(())
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

    fn leds(&self, on: &[bool; 8]) {
        if self.kind == Rhythm::OpenEphys {
            let bits = on.iter().enumerate().fold(0, |bits, (i, &on)| bits | ((on as u32) << i));
            self.fp.wire(WIRE_IN_LED_DISPLAY, bits, 0xff);
        }
    }

    fn set_continuous(&self, on: bool) {
        self.fp.wire(WIRE_IN_RESET_RUN, if on { 0x02 } else { 0 }, 0x02);
    }

    fn set_max_time_step(&self, steps: u32) {
        self.fp.set(WIRE_IN_MAX_TIME_STEP_LSB, steps & 0xffff, 0xffff_ffff);
        self.fp.set(WIRE_IN_MAX_TIME_STEP_MSB, steps >> 16, 0xffff_ffff);
        self.fp.update_ins();
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
        })?;
        self.sample_rate = rate;
        self.registers.set_sample_rate(rate as f64);
        Ok(())
    }

    fn upload_command_list(&self, list: &[u32], slot: usize, bank: u32) {
        for (i, &command) in list.iter().enumerate() {
            self.fp.set(WIRE_IN_CMD_RAM_DATA, command, 0xffff_ffff);
            self.fp.set(WIRE_IN_CMD_RAM_ADDR, i as u32, 0xffff_ffff);
            self.fp.set(WIRE_IN_CMD_RAM_BANK, bank, 0xffff_ffff);
            self.fp.update_ins();
            self.fp.trigger(TRIG_IN_RAM_WRITE, slot as c_int);
        }
    }

    fn select_aux_bank_all(&self, slot: usize, bank: u32) {
        for port in 0..4 {
            self.fp.set(WIRE_IN_AUX_CMD_BANK[slot], bank << (4 * port), 0x000f << (4 * port));
        }
        self.fp.update_ins();
    }

    fn select_aux_length(&self, slot: usize, loop_index: u32, end_index: u32) {
        self.fp.set(WIRE_IN_AUX_CMD_LOOP[slot], loop_index, 0xffff_ffff);
        self.fp.set(WIRE_IN_AUX_CMD_LENGTH[slot], end_index, 0xffff_ffff);
        self.fp.update_ins();
    }

    fn set_cable_delay(&mut self, port: usize, delay: u32) {
        let delay = delay.min(15);
        self.delays[port] = delay;
        self.fp.wire(WIRE_IN_MISO_DELAY, delay << (4 * port), 0x000f << (4 * port));
    }

    /// The cable delay for a cable `meters` long, at the sample rate.
    fn set_cable_length(&mut self, port: usize, meters: f64) {
        let t_step = 1.0 / (2800.0 * self.sample_rate as f64);
        let velocity = 0.555 * 299_792_458.0;
        let delay = 2.0 * meters / velocity + 1.9e-9 + 9.0e-9 + 1.4e-9 + 6.7e-9;
        self.set_cable_delay(port, ((delay / t_step + 1.0) + 0.5).floor().max(1.0) as u32);
    }

    fn set_data_source(&self, stream: u32, source: u32) {
        let endpoint = if (stream % 8) < 4 { WIRE_IN_DATA_STREAM_SEL_1234 } else { WIRE_IN_DATA_STREAM_SEL_5678 };
        let shift = 4 * (stream % 4) + if stream >= 8 { 16 } else { 0 };
        self.fp.wire(endpoint, source << shift, 0x000f << shift);
    }

    /// Enables the first `count` data streams, and no others.
    fn enable_streams(&mut self, count: u32) {
        let bits = if count >= 32 { u32::MAX } else { (1 << count) - 1 };
        self.fp.wire(WIRE_IN_DATA_STREAM_EN, bits, (1 << self.max_streams()) - 1);
        self.enabled_streams = count;
    }

    /// Bytes in `samples` samples of the enabled streams.
    fn bytes(&self, samples: usize) -> usize {
        2 * samples * (4 + 2 + 36 * self.enabled_streams as usize + 8 + 2)
    }

    /// Reads `samples` samples into `buf` (the board must have them, or be about to).
    fn read_samples(&mut self, samples: usize) -> io::Result<()> {
        let bytes = self.bytes(samples);
        self.buf.resize(bytes, 0);
        let (api, handle) = (self.fp.api, self.fp.handle);
        let got = unsafe {
            if self.usb3 {
                (api.okFrontPanel_ReadFromBlockPipeOut)(
                    handle,
                    PIPE_OUT_DATA,
                    USB3_BLOCK_SIZE,
                    bytes as c_long,
                    self.buf.as_mut_ptr(),
                )
            } else {
                (api.okFrontPanel_ReadFromPipeOut)(handle, PIPE_OUT_DATA, bytes as c_long, self.buf.as_mut_ptr())
            }
        };
        if got < 0 {
            return Err(io::Error::other(format!("reading from the board failed (error {got})")));
        }
        Ok(())
    }

    /// Runs `samples` samples, then reads them.
    fn run_once(&mut self, samples: u32) -> io::Result<()> {
        self.set_max_time_step(samples);
        self.set_continuous(false);
        self.run();
        self.wait("finish running", |b| !b.is_running())?;
        self.read_samples(samples as usize)
    }

    /// Word `word` (counting from the sample's start) of sample `t` in `buf`.
    fn word(&self, t: usize, word: usize) -> u16 {
        let at = self.bytes(t) + 2 * word;
        u16::from_le_bytes([self.buf[at], self.buf[at + 1]])
    }

    /// Word offset, within a sample, of auxiliary command slot `slot`'s result for stream `stream`.
    fn aux_word(&self, slot: usize, stream: usize) -> usize {
        6 + slot * self.enabled_streams as usize + stream
    }

    /// Uploads the command lists for the sample rate, bandwidth and DSP: the third slot's bank 0 calibrates,
    /// bank 1 runs normally.
    fn upload_registers(&mut self, bandwidth: &super::Bandwidth) -> (f64, f64, Option<f64>) {
        self.registers.set_dig_out_low();
        let list = self.registers.update_dig_out();
        self.upload_command_list(&list, 0, 0);
        self.select_aux_length(0, 0, list.len() as u32 - 1);
        self.select_aux_bank_all(0, 0);
        let list = self.registers.temperature_sensor();
        self.upload_command_list(&list, 1, 0);
        self.select_aux_length(1, 0, list.len() as u32 - 1);
        self.select_aux_bank_all(1, 0);

        let dsp = bandwidth.dsp_hz.map(|hz| self.registers.set_dsp_cutoff(hz));
        self.registers.enable_dsp(dsp.is_some());
        let lower = self.registers.set_lower_bandwidth(bandwidth.lower_hz);
        let upper = self.registers.set_upper_bandwidth(bandwidth.upper_hz);
        self.registers.enable_aux(true);
        self.registers.set_fast_settle(false);
        let list = self.registers.register_config(true);
        self.upload_command_list(&list, 2, 0);
        let list = self.registers.register_config(false);
        self.upload_command_list(&list, 2, 1);
        self.select_aux_length(2, 0, list.len() as u32 - 1);
        self.registers.set_fast_settle(true);
        let list = self.registers.register_config(false);
        self.upload_command_list(&list, 2, 2);
        self.registers.set_fast_settle(false);
        self.select_aux_bank_all(2, 1);
        (lower, upper, dsp)
    }

    /// Runs the calibrating command list once, as the board starts.
    fn calibrate(&mut self) -> io::Result<()> {
        self.upload_registers(&super::Bandwidth::default());
        self.select_aux_bank_all(2, 0);
        self.run_once(self.scan_samples())?;
        self.select_aux_bank_all(2, 1);
        Ok(())
    }

    fn scan_samples(&self) -> u32 {
        if self.usb3 { SCAN_SAMPLES_USB3 } else { SCAN_SAMPLES_USB2 }
    }

    /// Finds which chip is on each data source, and each port's best cable delay: at each delay, reads every
    /// source's chip ID back; the delays at which a chip answers are good, and the second good one is best (the
    /// first if there are only one or two).
    fn scan(&mut self) -> io::Result<()> {
        for (stream, source) in (0..8).enumerate() {
            self.set_data_source(stream as u32, source);
        }
        self.enable_streams(8);
        self.select_aux_bank_all(2, 0);
        let mut good: [Vec<u32>; 8] = Default::default();
        let mut chips = [None; 8];
        for delay in 0..16 {
            for port in 0..4 {
                self.set_cable_delay(port, delay);
            }
            self.run_once(self.scan_samples())?;
            for (source, good) in good.iter_mut().enumerate() {
                let results: Vec<u16> = (0..60).map(|t| self.word(t, self.aux_word(2, source))).collect();
                let Some((id, register_59)) = rhd::chip_id(&results) else { continue };
                if id == CHIP_RHD2132 || id == CHIP_RHD2216 || (id == CHIP_RHD2164 && register_59 == REGISTER_59_MISO_A)
                {
                    good.push(delay);
                    chips[source] = Some(id);
                }
            }
        }
        self.chips = chips;
        for port in 0..4 {
            let best = |source: usize| match good[source].len() {
                0 => 0,
                1 | 2 => good[source][0],
                _ => good[source][1],
            };
            self.set_cable_delay(port, best(2 * port).max(best(2 * port + 1)));
        }
        self.select_aux_bank_all(2, 1);
        Ok(())
    }

    /// The headstages scanning found, at their full channel counts.
    fn found_headstages(&self) -> Vec<Headstage> {
        let mut headstages = Vec::new();
        for (source, chip) in self.chips.iter().enumerate() {
            let channels = match chip {
                Some(CHIP_RHD2164) => 64,
                Some(CHIP_RHD2132) => 32,
                Some(CHIP_RHD2216) => 16,
                _ => continue,
            };
            headstages.push(Headstage { port: SOURCES[source].into(), channels });
        }
        headstages
    }

    /// The data streams to read for `wanted` headstages, if the board has them (an RHD2132 can be read as 16
    /// channels, an RHD2164 as 32); otherwise for every headstage found, at full channel counts.
    fn choose_streams(&self, wanted: &[Headstage]) -> (Vec<Stream>, Option<String>) {
        let streams_for = |headstages: &[Headstage]| -> Option<Vec<Stream>> {
            let mut streams = Vec::new();
            for headstage in headstages {
                let source = SOURCES.iter().position(|s| *s == headstage.port)? as u32;
                let chip = self.chips[source as usize]?;
                let stream =
                    |second, channels| Stream { source: source + if second { DDR } else { 0 }, second, chip, channels };
                match (chip, headstage.channels) {
                    (CHIP_RHD2164, 64) => streams.extend([stream(false, 32), stream(true, 32)]),
                    (CHIP_RHD2164, 32) | (CHIP_RHD2132, 32) => streams.push(stream(false, 32)),
                    (CHIP_RHD2132, 16) | (CHIP_RHD2216, 16) => streams.push(stream(false, 16)),
                    _ => return None,
                }
            }
            (streams.len() <= self.max_streams() as usize).then_some(streams)
        };
        match streams_for(wanted) {
            Some(streams) => (streams, None),
            None => {
                let found = self.found_headstages();
                let mut streams = streams_for(&found).unwrap_or_default();
                streams.truncate(self.max_streams() as usize);
                let note = format!(
                    "The board's headstages are {}, so it's acquiring all those rather than the {} set",
                    Headstage::describe(&found),
                    Headstage::describe(wanted)
                );
                (streams, Some(note))
            }
        }
    }

    fn flush(&mut self) {
        let limit = Instant::now() + Duration::from_secs(2);
        loop {
            let words = self.words_in_fifo() as usize;
            if words == 0 || Instant::now() > limit {
                break;
            }
            let bytes = (2 * words).min(1 << 20);
            let bytes =
                if self.usb3 { bytes.div_ceil(USB3_BLOCK_SIZE as usize) * USB3_BLOCK_SIZE as usize } else { bytes };
            self.buf.resize(bytes, 0);
            let (api, handle) = (self.fp.api, self.fp.handle);
            unsafe {
                if self.usb3 {
                    (api.okFrontPanel_ReadFromBlockPipeOut)(
                        handle,
                        PIPE_OUT_DATA,
                        USB3_BLOCK_SIZE,
                        bytes as c_long,
                        self.buf.as_mut_ptr(),
                    );
                } else {
                    (api.okFrontPanel_ReadFromPipeOut)(handle, PIPE_OUT_DATA, bytes as c_long, self.buf.as_mut_ptr());
                }
            }
        }
    }

    fn stop(&mut self) {
        if self.running {
            self.set_continuous(false);
            self.set_max_time_step(0);
            self.flush();
            self.leds(&[true, false, false, false, false, false, false, false]);
            self.running = false;
        }
    }
}

impl Drop for OpalKellyBoard {
    fn drop(&mut self) {
        self.stop();
        self.leds(&[false; 8]);
        unsafe { (self.fp.api.okFrontPanel_ResetFPGA)(self.fp.handle) };
    }
}

impl Board for OpalKellyBoard {
    fn info(&self) -> &BoardInfo {
        &self.info
    }

    fn start(&mut self, config: &Config) -> io::Result<()> {
        if !SAMPLE_RATES.contains(&config.sample_rate) {
            return Err(io::Error::other(format!("{} Hz isn't one of the board's sample rates", config.sample_rate)));
        }
        let (streams, note) = self.choose_streams(&config.headstages);
        self.warnings.clear();
        self.warnings.extend(note);
        self.set_sample_rate(config.sample_rate)?;
        // Cable delays depend on the sample rate.
        if config.sample_rate != 30000 {
            self.scan()?;
        }
        for (stream, s) in streams.iter().enumerate() {
            self.set_data_source(stream as u32, s.source);
        }
        self.enable_streams(streams.len() as u32);
        self.streams = streams;
        let (lower, upper, dsp) = self.upload_registers(&config.bandwidth);
        let near = |got: f64, asked: f64| (got - asked).abs() <= 0.05 * asked;
        if !near(lower, config.bandwidth.lower_hz) || !near(upper, config.bandwidth.upper_hz) {
            self.warnings.push(format!("The chips' bandwidth is {lower:.2} to {upper:.0} Hz, as near as they get"));
        }
        if let (Some(dsp), Some(asked)) = (dsp, config.bandwidth.dsp_hz)
            && !near(dsp, asked)
        {
            self.warnings.push(format!("The DSP high-pass is at {dsp:.2} Hz, as near as it gets"));
        }
        self.aux = vec![[0.0; 3]; self.streams.len()];
        self.aux_due = vec![[0.0; 3]; self.streams.len()];
        self.leds(&[true, true, false, false, false, false, false, false]);
        self.flush();
        self.set_continuous(true);
        self.run();
        self.running = true;
        Ok(())
    }

    fn read(&mut self, block: &mut Block) -> io::Result<bool> {
        let samples = self.samples_per_block();
        if !self.usb3 && (self.words_in_fifo() as usize) < self.bytes(samples) / 2 {
            std::thread::sleep(Duration::from_millis(1));
            return Ok(false);
        }
        self.read_samples(samples)?;
        let streams = self.streams.clone();
        let ephys: usize = streams.iter().map(|s| s.channels).sum();
        let aux_streams: Vec<usize> = (0..streams.len()).filter(|&s| !streams[s].second).collect();
        block.resize(samples, ephys, 3 * aux_streams.len(), 8);
        let n = self.enabled_streams as usize;
        let mut first_timestamp = None;
        for t in 0..samples {
            let at = self.bytes(t);
            let header = u64::from_le_bytes(self.buf[at..at + 8].try_into().unwrap());
            if header != HEADER_MAGIC {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "lost track of the board's samples"));
            }
            if t == 0 {
                first_timestamp = Some(u32::from_le_bytes(self.buf[at + 8..at + 12].try_into().unwrap()));
            }
            // Headstage channels: 32 per stream, interleaved stream by stream.
            let mut channel = 0;
            for (stream, s) in streams.iter().enumerate() {
                let offset = if s.chip == CHIP_RHD2132 && s.channels == 16 { RHD2132_16CH_OFFSET } else { 0 };
                for c in 0..s.channels {
                    let raw = self.word(t, 6 + 3 * n + (offset + c) * n + stream);
                    block.ephys[channel * samples + t] = (raw as f32 - 32768.0) * EPHYS_MICROVOLTS_PER_BIT;
                    channel += 1;
                }
            }
            // Auxiliary inputs, from the second command slot, each sampled every fourth sample (and held).
            let which = (t + 3) % 4;
            for (i, &stream) in aux_streams.iter().enumerate() {
                if which < 3 {
                    let raw = self.word(t, self.aux_word(1, stream));
                    self.aux_due[stream][which] = (raw as f32 - 32768.0) * AUX_VOLTS_PER_BIT;
                } else {
                    self.aux[stream] = self.aux_due[stream];
                }
                for input in 0..3 {
                    block.aux[(3 * i + input) * samples + t] = self.aux[stream][input];
                }
            }
            let adc = 6 + 36 * n;
            for input in 0..8 {
                block.adc[input * samples + t] = self.kind.adc_volts(self.word(t, adc + input));
            }
            block.ttl[t] = self.word(t, adc + 8) as u32;
        }
        block.first = first_timestamp.unwrap_or(0) as u64;
        Ok(true)
    }

    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
}
