//! The generation 3 Open Ephys Acquisition Board, which runs Rhythm firmware behind an ONI (Open Neuro
//! Interface) controller over USB 3 (an FTDI FT600). Ported from the Open Ephys acquisition board plugin
//! (`Rhd2000ONIBoard`, `AcqBoardONI`); liboni, the ONI host library, is ported in `liboni`.
//!
//! liboni's FT600 driver, `libonidriver_ft600`, is a library loaded at runtime, as it's built on FTDI's own: it
//! comes with the program, and is otherwise looked for where the Open Ephys GUI puts it. On Windows, FTDI's D3XX
//! USB driver must be installed too.
//!
//! This is the transport: the rest is in `rhythm`, shared with the earlier boards. Not yet tried on hardware.

use std::ffi::{c_char, c_int, c_void};
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use libloading::Library;

use super::liboni::{self, Context, Driver, Stream};
use super::opal_kelly::search_dirs;
use super::rhythm::{RhythmBoard, Transport, sample_bytes};

/// liboni's driver for the board's USB 3 chip.
#[cfg(windows)]
const DRIVER_NAME: &str = "libonidriver_ft600.dll";
#[cfg(target_os = "macos")]
const DRIVER_NAME: &str = "libonidriver_ft600.dylib";
#[cfg(all(unix, not(target_os = "macos")))]
const DRIVER_NAME: &str = "libonidriver_ft600.so";

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

type DriverCtx = *mut c_void;

macro_rules! driver_api {
    ($($name:ident($($arg:ty),*) -> $ret:ty;)*) => {
        #[allow(non_snake_case)]
        struct DriverApi {
            $($name: unsafe extern "C" fn($($arg),*) -> $ret,)*
            _library: Library,
        }

        impl DriverApi {
            /// # Safety
            /// `library` must be a liboni driver, whose functions have these signatures.
            unsafe fn from_library(library: Library) -> Result<DriverApi, String> {
                unsafe {
                    Ok(DriverApi {
                        $($name: *library
                            .get::<unsafe extern "C" fn($($arg),*) -> $ret>(stringify!($name))
                            .map_err(|e| format!("the FT600 driver is missing {}: {e}", stringify!($name)))?,)*
                        _library: library,
                    })
                }
            }
        }
    };
}

driver_api! {
    oni_driver_create_ctx() -> DriverCtx;
    oni_driver_destroy_ctx(DriverCtx) -> c_int;
    oni_driver_init(DriverCtx, c_int) -> c_int;
    oni_driver_read_stream(DriverCtx, c_int, *mut c_void, usize) -> c_int;
    oni_driver_write_stream(DriverCtx, c_int, *const c_char, usize) -> c_int;
    oni_driver_read_config(DriverCtx, c_int, *mut u32) -> c_int;
    oni_driver_write_config(DriverCtx, c_int, u32) -> c_int;
    oni_driver_set_opt_callback(DriverCtx, c_int, *const c_void, usize) -> c_int;
}

impl DriverApi {
    /// Loads the driver once it's found (it's never unloaded); until then, tries again each time.
    fn get(path: Option<&str>) -> io::Result<&'static DriverApi> {
        static API: OnceLock<DriverApi> = OnceLock::new();
        if let Some(api) = API.get() {
            return Ok(api);
        }
        let candidates: Vec<PathBuf> = match path {
            Some(p) => vec![p.into()],
            None => search_dirs()
                .into_iter()
                .map(|d| d.join(DRIVER_NAME))
                .chain(std::iter::once(DRIVER_NAME.into()))
                .collect(),
        };
        let mut errors = Vec::new();
        for candidate in &candidates {
            // Safety: loading a library runs its initialisers; the driver's are benign.
            match unsafe { Library::new(candidate) } {
                Ok(library) => {
                    // Safety: it's the FT600 driver, by name.
                    let api = unsafe { DriverApi::from_library(library) }.map_err(io::Error::other)?;
                    return Ok(API.get_or_init(|| api));
                }
                Err(e) => errors.push(format!("{}: {e}", candidate.display())),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("liboni's FT600 driver ({DRIVER_NAME}) not found. Tried {}", errors.join("; ")),
        ))
    }
}

/// The FT600 driver, with a context of its own.
struct Ft600 {
    api: &'static DriverApi,
    ctx: DriverCtx,
}

impl Ft600 {
    fn new(api: &'static DriverApi) -> io::Result<Ft600> {
        let ctx = unsafe { (api.oni_driver_create_ctx)() };
        if ctx.is_null() {
            return Err(io::Error::other("the FT600 driver couldn't make a context"));
        }
        Ok(Ft600 { api, ctx })
    }
}

impl Driver for Ft600 {
    fn init(&mut self, host_index: i32) -> liboni::Result<()> {
        liboni::check(unsafe { (self.api.oni_driver_init)(self.ctx, host_index) }).map(drop)
    }

    fn read_stream(&mut self, stream: Stream, data: &mut [u8]) -> liboni::Result<usize> {
        let code = unsafe {
            (self.api.oni_driver_read_stream)(self.ctx, stream as c_int, data.as_mut_ptr().cast(), data.len())
        };
        liboni::check(code).map(|n| n as usize)
    }

    fn write_stream(&mut self, data: &[u8]) -> liboni::Result<usize> {
        let code = unsafe { (self.api.oni_driver_write_stream)(self.ctx, 0, data.as_ptr().cast(), data.len()) };
        liboni::check(code).map(|n| n as usize)
    }

    fn read_config(&mut self, register: u32) -> liboni::Result<u32> {
        let mut value = 0;
        liboni::check(unsafe { (self.api.oni_driver_read_config)(self.ctx, register as c_int, &mut value) })?;
        Ok(value)
    }

    fn write_config(&mut self, register: u32, value: u32) -> liboni::Result<()> {
        liboni::check(unsafe { (self.api.oni_driver_write_config)(self.ctx, register as c_int, value) }).map(drop)
    }

    fn set_opt_callback(&mut self, option: i32, value: u32) -> liboni::Result<()> {
        let value: *const u32 = &value;
        let code = unsafe { (self.api.oni_driver_set_opt_callback)(self.ctx, option, value.cast(), size_of::<u32>()) };
        liboni::check(code).map(drop)
    }
}

impl Drop for Ft600 {
    fn drop(&mut self) {
        unsafe { (self.api.oni_driver_destroy_ctx)(self.ctx) };
    }
}

// The driver's context is only used from the thread that owns the board.
unsafe impl Send for Ft600 {}

/// Talks to a generation 3 board through liboni.
pub struct Oni {
    context: Context<Ft600>,
    device_id: u32,
    enabled_streams: u32,
    running: bool,
}

/// Opens the board. `driver` overrides where liboni's FT600 driver is looked for.
pub fn open(driver: Option<&str>) -> io::Result<RhythmBoard<Oni>> {
    let mut context = Context::new(Ft600::new(DriverApi::get(driver)?)?);
    match context.init(-1) {
        Ok(()) => {}
        Err(liboni::Error(liboni::EBADCONTROLLER)) => {
            return Err(io::Error::other("the board's gateware needs updating (to 2.0.0 or later) for this software"));
        }
        Err(e) => return Err(io::Error::other(format!("opening the generation 3 board failed: {e}"))),
    }
    let mut board = Oni { context, device_id: 0, enabled_streams: 1, running: false };
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
    fn read_register(&mut self, device: u32, register: u32) -> io::Result<u32> {
        self.context.read_reg(device, register).map_err(|e| io::Error::other(format!("reading a register failed: {e}")))
    }

    fn write_register(&mut self, device: u32, register: u32, value: u32) {
        let _ = self.context.write_reg(device, register, value);
    }

    /// Writes the bits of `mask`, keeping the rest.
    fn write_masked(&mut self, device: u32, register: u32, value: u32, mask: u32) {
        if let Ok(old) = self.read_register(device, register) {
            self.write_register(device, register, (old & !mask) | (value & mask));
        }
    }

    /// Resets the controller (needed whenever the frames' size changes).
    fn reset(&mut self) {
        std::thread::sleep(Duration::from_millis(50));
        let _ = self.context.reset();
        let _ = self.context.set_block_read_size(BLOCK_READ_SIZE);
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

    fn write_ttl_out(&mut self, lines: u32) {
        let _ = self.context.write_frame(DEVICE_TTL, &lines.to_le_bytes());
    }

    fn run(&mut self) {
        let _ = self.context.reset_acq_counter(2);
    }

    fn halt(&mut self) {
        let _ = self.context.set_running(false);
    }

    /// Reads frames until one from the Rhythm device comes, appending its sample to `buf`.
    fn read_sample(&mut self, buf: &mut Vec<u8>) -> io::Result<()> {
        let expected = sample_bytes(self.enabled_streams);
        loop {
            let frame = self
                .context
                .read_frame()
                .map_err(|e| io::Error::other(format!("reading from the board failed: {e}")))?;
            if frame.dev_idx != DEVICE_RHYTHM {
                continue;
            }
            if frame.data.len() != 8 + expected {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "a sample of the wrong size from the board"));
            }
            buf.extend_from_slice(&frame.data[8..]); // after the hub's clock
            return Ok(());
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
    }
}
