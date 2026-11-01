//! The generation 3 Open Ephys Acquisition Board, which runs Rhythm firmware behind an ONI (Open Neuro
//! Interface) controller over USB 3 (an FTDI FT600). Ported from the Open Ephys acquisition board plugin
//! (`Rhd2000ONIBoard`, `AcqBoardONI`).
//!
//! liboni (and its FT600 driver, `onidriver_ft600`, which it loads in turn) is loaded at runtime: it comes with
//! the Open Ephys GUI's acquisition board plugin, and is looked for where the GUI puts it. FTDI's D3XX driver
//! must be installed too.
//!
//! This is the transport: the rest is in `rhythm`, shared with the earlier boards. Not yet tried on hardware.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use libloading::Library;

use super::opal_kelly::search_dirs;
use super::rhythm::{RhythmBoard, Transport, sample_bytes};

type Ctx = *mut c_void;

/// A frame from liboni.
#[repr(C)]
struct Frame {
    time: u64,
    dev_idx: u32,
    data_sz: u32,
    data: *mut c_char,
}

#[cfg(windows)]
const LIBRARY_NAME: &str = "liboni.dll";
#[cfg(target_os = "macos")]
const LIBRARY_NAME: &str = "liboni.dylib";
#[cfg(all(unix, not(target_os = "macos")))]
const LIBRARY_NAME: &str = "liboni.so";

const ONI_OPT_RUNNING: c_int = 2;
const ONI_OPT_RESET: c_int = 3;
const ONI_OPT_RESETACQCOUNTER: c_int = 6;
const ONI_OPT_BLOCKREADSIZE: c_int = 10;
const ONI_EBADCONTROLLER: c_int = -29;
const BLOCK_READ_SIZE: u32 = 24 * 1024;

const DEVICE_INFO: u32 = 254;
const DEVICE_RHYTHM: u32 = 0x0101;
const DEVICE_TTL: u32 = 0x0102;
const HUB_MANAGER: u32 = 0x01fe;
const HUB_CLOCK_SEL: u32 = 0x2000;
const HUB_CLOCK_BUSY: u32 = 0x2001;
/// The board's revision with inverted analogue inputs.
const DEVICE_ID_V3: u32 = 0x0102;
/// Where command lists go, in the Rhythm device's registers.
const COMMAND_RAM: u32 = 0x4000;

// The Rhythm device's registers.
const MODE: u32 = 1;
const MAX_TIMESTEP: u32 = 2;
const CABLE_DELAY: u32 = 3;
const AUXCMD_BANK: [u32; 3] = [4, 5, 6];
const MAX_AUXCMD_INDEX: [u32; 3] = [7, 8, 9];
const LOOP_AUXCMD_INDEX: [u32; 3] = [10, 11, 12];
const DATA_STREAM_SEL: [u32; 2] = [13, 14];
const DATA_STREAM_EN: u32 = 15;
const EXTERNAL_FAST_SETTLE: u32 = 16;
const EXTERNAL_DIGOUT: [u32; 4] = [17, 18, 19, 20];
const DAC_CTL: u32 = 22;
const DAC_SEL: [u32; 8] = [23, 24, 25, 26, 27, 28, 29, 30];
const DAC_THRESH: [u32; 8] = [31, 32, 33, 34, 35, 36, 37, 38];
// Bits of the mode register.
const SPI_RUN_CONTINUOUS: u32 = 1;
const DSP_SETTLE: u32 = 2;
const TTL_OUT_MODE: u32 = 3;
const LED_ENABLE: u32 = 4;

/// Frames read at a time while running.
const FRAMES_PER_READ: usize = 128;

macro_rules! oni_api {
    ($($name:ident($($arg:ty),*) -> $ret:ty;)*) => {
        #[allow(non_snake_case)]
        struct Api {
            $($name: unsafe extern "C" fn($($arg),*) -> $ret,)*
            _library: Library,
        }

        impl Api {
            /// # Safety
            /// `library` must be liboni, whose functions have these signatures.
            unsafe fn from_library(library: Library) -> Result<Api, String> {
                unsafe {
                    Ok(Api {
                        $($name: *library
                            .get::<unsafe extern "C" fn($($arg),*) -> $ret>(stringify!($name))
                            .map_err(|e| format!("liboni is missing {}: {e}", stringify!($name)))?,)*
                        _library: library,
                    })
                }
            }
        }
    };
}

oni_api! {
    oni_create_ctx(*const c_char) -> Ctx;
    oni_init_ctx(Ctx, c_int) -> c_int;
    oni_destroy_ctx(Ctx) -> c_int;
    oni_set_opt(Ctx, c_int, *const c_void, usize) -> c_int;
    oni_read_reg(Ctx, u32, u32, *mut u32) -> c_int;
    oni_write_reg(Ctx, u32, u32, u32) -> c_int;
    oni_read_frame(Ctx, *mut *mut Frame) -> c_int;
    oni_create_frame(Ctx, *mut *mut Frame, u32, *const c_void, usize) -> c_int;
    oni_write_frame(Ctx, *const Frame) -> c_int;
    oni_destroy_frame(*mut Frame) -> ();
    oni_error_str(c_int) -> *const c_char;
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
            // Safety: loading a library runs its initialisers; liboni's are benign.
            match unsafe { Library::new(candidate) } {
                Ok(library) => {
                    // Safety: it's liboni, by name.
                    let api = unsafe { Api::from_library(library) }.map_err(io::Error::other)?;
                    return Ok(API.get_or_init(|| api));
                }
                Err(e) => errors.push(format!("{}: {e}", candidate.display())),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "liboni not found (it comes with the Open Ephys GUI's acquisition board plugin). Tried {}",
                errors.join("; ")
            ),
        ))
    }

    fn error(&self, code: c_int, what: &str) -> io::Error {
        let message = unsafe { CStr::from_ptr((self.oni_error_str)(code)) }.to_string_lossy().into_owned();
        io::Error::other(format!("{what} failed: {message} ({code})"))
    }
}

/// Talks to a generation 3 board through liboni.
pub struct Oni {
    api: &'static Api,
    ctx: Ctx,
    device_id: u32,
    enabled_streams: u32,
    running: bool,
}

// The context is only used from the thread that owns the board.
unsafe impl Send for Oni {}

/// Opens the board. `library` overrides where liboni is looked for.
pub fn open(library: Option<&str>) -> io::Result<RhythmBoard<Oni>> {
    let api = Api::get(library)?;
    let driver = CString::new("ft600").unwrap();
    let ctx = unsafe { (api.oni_create_ctx)(driver.as_ptr()) };
    if ctx.is_null() {
        return Err(io::Error::other("liboni couldn't load its FT600 driver (onidriver_ft600)"));
    }
    let code = unsafe { (api.oni_init_ctx)(ctx, -1) };
    if code != 0 {
        unsafe { (api.oni_destroy_ctx)(ctx) };
        return Err(if code == ONI_EBADCONTROLLER {
            io::Error::other("the board's gateware needs updating (to 2.0.0 or later) for this software")
        } else {
            api.error(code, "opening the generation 3 board")
        });
    }
    let mut board = Oni { api, ctx, device_id: 0, enabled_streams: 1, running: false };
    // Memory on the board must be ready first.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match board.read_register(DEVICE_INFO, 0x1000).map(|v| v & 0x03) {
            Ok(1) => break,
            Ok(0) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(0) => return Err(io::Error::new(io::ErrorKind::TimedOut, "the board's memory didn't initialise")),
            Ok(_) => return Err(io::Error::other("the board reports a memory error")),
            Err(e) => return Err(e),
        }
    }
    board.device_id = board.read_register(DEVICE_INFO, 0).unwrap_or(0);
    board.initialize()?;
    RhythmBoard::new(board)
}

impl Oni {
    fn read_register(&self, device: u32, register: u32) -> io::Result<u32> {
        let mut value = 0;
        let code = unsafe { (self.api.oni_read_reg)(self.ctx, device, register, &mut value) };
        if code != 0 {
            return Err(self.api.error(code, "reading a register"));
        }
        Ok(value)
    }

    fn write_register(&self, device: u32, register: u32, value: u32) {
        unsafe { (self.api.oni_write_reg)(self.ctx, device, register, value) };
    }

    /// Writes the bits of `mask`, keeping the rest.
    fn write_masked(&self, device: u32, register: u32, value: u32, mask: u32) {
        if let Ok(old) = self.read_register(device, register) {
            self.write_register(device, register, (old & !mask) | (value & mask));
        }
    }

    fn set_opt(&self, option: c_int, value: u32) {
        unsafe { (self.api.oni_set_opt)(self.ctx, option, &value as *const u32 as *const c_void, size_of::<u32>()) };
    }

    /// Resets the controller (needed whenever the frames' size changes).
    fn reset(&self) {
        std::thread::sleep(Duration::from_millis(50));
        self.set_opt(ONI_OPT_RESET, 1);
        self.set_opt(ONI_OPT_BLOCKREADSIZE, BLOCK_READ_SIZE);
    }

    /// The firmware's settings that aren't the board's business (DACs, digital outputs...), as the plugin's
    /// `initialize` leaves them; the rest is `RhythmBoard`'s.
    fn initialize(&mut self) -> io::Result<()> {
        self.reset();
        self.write_masked(DEVICE_RHYTHM, MODE, 0, 1 << DSP_SETTLE);
        self.write_ttl_out(0);
        for dac in 0..8 {
            self.write_masked(DEVICE_RHYTHM, DAC_SEL[dac], 0, 0x0400 | (0x1f << 5) | 0x1f);
            self.write_register(DEVICE_RHYTHM, DAC_THRESH[dac], 32768 | (1 << 16));
        }
        self.write_masked(DEVICE_RHYTHM, DAC_CTL, 0, (0x07 << 7) | 0x7f);
        self.write_masked(DEVICE_RHYTHM, MODE, 1 << TTL_OUT_MODE, 1 << TTL_OUT_MODE);
        self.write_masked(DEVICE_RHYTHM, EXTERNAL_FAST_SETTLE, 0, 0x1f);
        for register in EXTERNAL_DIGOUT {
            self.write_masked(DEVICE_RHYTHM, register, 0, 0x1f);
        }
        self.write_masked(DEVICE_RHYTHM, MODE, 1 << LED_ENABLE, 1 << LED_ENABLE);
        Ok(())
    }

    fn write_ttl_out(&self, lines: u32) {
        let mut frame: *mut Frame = std::ptr::null_mut();
        let data = lines as i32;
        let code = unsafe {
            (self.api.oni_create_frame)(self.ctx, &mut frame, DEVICE_TTL, &data as *const i32 as *const c_void, 4)
        };
        if code >= 0 && !frame.is_null() {
            unsafe {
                (self.api.oni_write_frame)(self.ctx, frame);
                (self.api.oni_destroy_frame)(frame);
            }
        }
    }

    fn run(&self) {
        self.set_opt(ONI_OPT_RESETACQCOUNTER, 2);
    }

    fn halt(&self) {
        self.set_opt(ONI_OPT_RUNNING, 0);
    }

    /// Reads frames until one from the Rhythm device comes, appending its sample to `buf`.
    fn read_sample(&self, buf: &mut Vec<u8>) -> io::Result<()> {
        let expected = sample_bytes(self.enabled_streams);
        loop {
            let mut frame: *mut Frame = std::ptr::null_mut();
            let code = unsafe { (self.api.oni_read_frame)(self.ctx, &mut frame) };
            if code < 0 {
                return Err(self.api.error(code, "reading from the board"));
            }
            // Safety: liboni gave us this frame, and it's destroyed below.
            let (device, data) = unsafe {
                let f = &*frame;
                (f.dev_idx, std::slice::from_raw_parts(f.data as *const u8, f.data_sz as usize))
            };
            let result = if device != DEVICE_RHYTHM {
                None
            } else if data.len() != 8 + expected {
                Some(Err(io::Error::new(io::ErrorKind::InvalidData, "a sample of the wrong size from the board")))
            } else {
                buf.extend_from_slice(&data[8..]); // after the hub's clock
                Some(Ok(()))
            };
            unsafe { (self.api.oni_destroy_frame)(frame) };
            if let Some(result) = result {
                return result;
            }
        }
    }
}

impl Transport for Oni {
    fn description(&self) -> String {
        "Open Ephys Acquisition Board (gen 3)".into()
    }

    fn max_streams(&self) -> u32 {
        16
    }

    fn ttl_lines(&self) -> u32 {
        8
    }

    fn adc_volts(&self, raw: u16) -> f32 {
        if self.device_id == DEVICE_ID_V3 {
            -(0.000_152_587_89 * raw as f32 - 5.0)
        } else {
            0.000_152_587_89 * raw as f32 - 5.0 - 0.4096
        }
    }

    fn extra_delay(&self) -> u32 {
        1
    }

    fn set_sample_rate(&mut self, rate: u32) -> io::Result<()> {
        // The hub picks one of three clocks (84, 70 or 56 MHz, for 30, 25 and 20 kHz), divided or not.
        let (divide, clock, divider) = match rate {
            1000 => (1, 2, 9),
            1250 => (1, 1, 9),
            1500 => (1, 0, 9),
            2000 => (1, 2, 4),
            2500 => (1, 1, 4),
            3000 => (1, 0, 4),
            3333 => (1, 2, 2),
            5000 => (1, 2, 1),
            6250 => (1, 1, 1),
            10000 => (1, 2, 0),
            12500 => (1, 1, 0),
            15000 => (1, 0, 0),
            20000 => (0, 2, 0),
            25000 => (0, 1, 0),
            30000 => (0, 0, 0),
            _ => return Err(io::Error::other(format!("the generation 3 board can't sample at {rate} Hz"))),
        };
        self.write_register(HUB_MANAGER, HUB_CLOCK_SEL, ((divider & 0xf) << 3) + ((clock & 0x3) << 1) + divide);
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.read_register(HUB_MANAGER, HUB_CLOCK_BUSY)? != 0 {
            if Instant::now() > deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the board's clock didn't settle"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    fn upload_command_list(&mut self, list: &[u32], slot: usize, bank: u32) {
        for (i, &command) in list.iter().enumerate() {
            self.write_register(DEVICE_RHYTHM, COMMAND_RAM + (bank << 10) + ((slot as u32) << 14) + i as u32, command);
        }
    }

    fn select_aux_bank(&mut self, slot: usize, bank: u32) {
        for port in 0..4 {
            self.write_masked(DEVICE_RHYTHM, AUXCMD_BANK[slot], bank << (4 * port), 0x000f << (4 * port));
        }
    }

    fn select_aux_length(&mut self, slot: usize, end: u32) {
        self.write_register(DEVICE_RHYTHM, LOOP_AUXCMD_INDEX[slot], 0);
        self.write_register(DEVICE_RHYTHM, MAX_AUXCMD_INDEX[slot], end);
    }

    fn set_cable_delay(&mut self, port: usize, delay: u32) {
        self.write_masked(DEVICE_RHYTHM, CABLE_DELAY, delay.min(15) << (4 * port), 0x000f << (4 * port));
    }

    fn set_data_source(&mut self, stream: u32, source: u32) {
        let shift = 4 * (stream % 8);
        self.write_masked(DEVICE_RHYTHM, DATA_STREAM_SEL[(stream / 8) as usize], source << shift, 0x000f << shift);
    }

    fn enable_streams(&mut self, count: u32) {
        let bits = (1u32 << count) - 1;
        self.write_masked(DEVICE_RHYTHM, DATA_STREAM_EN, bits, 0xffff);
        self.enabled_streams = count;
        // The frames' size has changed.
        self.reset();
    }

    fn set_continuous(&mut self, on: bool) {
        self.write_masked(DEVICE_RHYTHM, MODE, (on as u32) << SPI_RUN_CONTINUOUS, 1 << SPI_RUN_CONTINUOUS);
    }

    fn set_max_time_step(&mut self, steps: u32) {
        self.write_register(DEVICE_RHYTHM, MAX_TIMESTEP, steps);
    }

    fn run_once(&mut self, samples: u32, buf: &mut Vec<u8>) -> io::Result<()> {
        self.set_max_time_step(samples.saturating_mul(128));
        self.set_continuous(false);
        self.run();
        buf.clear();
        let result = (0..samples).try_for_each(|_| self.read_sample(buf));
        self.halt();
        result
    }

    fn start(&mut self) -> io::Result<()> {
        self.set_continuous(true);
        self.run();
        self.running = true;
        Ok(())
    }

    fn read(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        buf.clear();
        for _ in 0..FRAMES_PER_READ {
            self.read_sample(buf)?;
        }
        Ok(FRAMES_PER_READ)
    }

    fn stop(&mut self) {
        if self.running {
            self.set_continuous(false);
            self.set_max_time_step(0);
            self.halt();
            self.reset();
            self.running = false;
        }
    }

    fn leds(&mut self, _running: bool) {}
}

impl Drop for Oni {
    fn drop(&mut self) {
        self.stop();
        unsafe { (self.api.oni_destroy_ctx)(self.ctx) };
    }
}
