//! The host side of ONI (the Open Neuro Interface), for the generation 3 board: ported from liboni
//! (github.com/open-ephys/liboni, `api/liboni/oni.c`, MIT licence: see liboni.LICENSE), as much of it as the board
//! uses.
//!
//! A controller has devices, each with registers and frames of its own. Below this is a driver, for the link to the
//! controller (the board's is liboni's FT600 driver, a library: see `oni.rs`). The driver gives configuration
//! registers, and two streams from the controller: data, the devices' frames; and signal, the controller's replies,
//! in COBS packets. A device's register is read or written by setting configuration registers and triggering, then
//! waiting for the reply. A reset has the controller send the device table, which says how big frames are.
//!
//! The specification is at github.com/open-ephys/ONI. This follows liboni, as the board's gateware and the FT600 driver
//! do, where the two differ: a read frame's header is its time, then device, then size; a register's value is read
//! back from the configuration registers, not from its acknowledgement; and frames' data is padded by the host.

use std::fmt;
use std::io;

// Configuration registers.
const CONFIG_DEV_IDX: u32 = 0;
const CONFIG_REG_ADDR: u32 = 1;
const CONFIG_REG_VALUE: u32 = 2;
const CONFIG_RW: u32 = 3;
const CONFIG_TRIG: u32 = 4;
const CONFIG_RUNNING: u32 = 5;
const CONFIG_RESET: u32 = 6;
const CONFIG_RESETACQCOUNTER: u32 = 9;

// Options, as the driver's told of them.
pub const OPT_RUNNING: i32 = 2;
pub const OPT_RESET: i32 = 3;
pub const OPT_RESETACQCOUNTER: i32 = 6;
pub const OPT_BLOCKREADSIZE: i32 = 10;

// The signal stream's packet types.
const CONFIGWACK: u32 = 1 << 1;
const CONFIGWNACK: u32 = 1 << 2;
const CONFIGRACK: u32 = 1 << 3;
const CONFIGRNACK: u32 = 1 << 4;
const DEVICETABLEACK: u32 = 1 << 5;
const DEVICEINST: u32 = 1 << 6;

/// A read frame's header: time (8 bytes), device index (4), data size (4).
const READ_HEADER: usize = 16;
/// A written frame's header: device index (4), data size in words (4).
const WRITE_HEADER: usize = 8;
/// The longest COBS packet.
const PACKET: usize = 255;
const WORD: usize = 4;

pub const EREADFAILURE: i32 = -5;
pub const EWRITEFAILURE: i32 = -6;
pub const EINVALSTATE: i32 = -9;
pub const EINVALARG: i32 = -11;
pub const ECOBSPACK: i32 = -12;
pub const ERETRIG: i32 = -13;
pub const EBUFFERSIZE: i32 = -14;
pub const EBADDEVTABLE: i32 = -15;
pub const EDEVIDX: i32 = -3;
pub const EWRITESIZE: i32 = -4;
pub const EINVALREADSIZE: i32 = -20;
pub const ENOTWRITEDEV: i32 = -25;
pub const EDEVIDXREPEAT: i32 = -26;
pub const EBADFRAME: i32 = -28;
pub const EBADCONTROLLER: i32 = -29;

/// An error, by liboni's code for it (which drivers use too).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error(pub i32);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let message = match self.0 {
            -1 => "invalid stream path",
            -2 => "invalid device ID",
            -3 => "invalid device index",
            -4 => "data size is not a multiple of the device's write size",
            -5 => "failure to read from a stream or register",
            -6 => "failure to write to a stream or register",
            -7 => "attempt to use a null context",
            -8 => "failure to seek on stream",
            -9 => "invalid operation for the current run state",
            -10 => "invalid context option",
            -11 => "invalid arguments",
            -12 => "invalid COBS packet",
            -13 => "a hardware operation was retriggered before the last finished",
            -14 => "supplied buffer is too small",
            -15 => "badly formatted device table supplied by firmware",
            -16 => "a memory allocation failed",
            -17 => "close failure",
            -18 => "attempted write to a read-only object",
            -19 => "unimplemented feature",
            -20 => "block read size is smaller than the largest frame",
            -21 => "frame read attempted with no readable devices",
            -22 => "hardware initialisation failed",
            -23 => "attempted read from a write-only object",
            -24 => "block write size is smaller than the largest frame",
            -25 => "frame written to a device that doesn't take them",
            -26 => "device table contains repeated device indices",
            -27 => "attempted to read or write a protected configuration option",
            -28 => "received malformed frame",
            -29 => "ONI controller is not compatible with the driver",
            _ => "unknown error",
        };
        write!(f, "{message} ({})", self.0)
    }
}

impl std::error::Error for Error {}

impl From<Error> for io::Error {
    fn from(e: Error) -> io::Error {
        io::Error::other(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Turns a driver's return code into a result, keeping what it returned if it's not an error.
pub fn check(code: i32) -> Result<i32> {
    if code < 0 { Err(Error(code)) } else { Ok(code) }
}

/// Which stream to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Data = 0,
    Signal = 1,
}

/// The link to the controller: liboni's driver functions.
pub trait Driver {
    fn init(&mut self, host_index: i32) -> Result<()>;
    /// Reads all of `data`, returning how much was read (less only if something went wrong), or an error.
    fn read_stream(&mut self, stream: Stream, data: &mut [u8]) -> Result<usize>;
    /// Writes `data` to the data stream, returning how much was written, or an error.
    fn write_stream(&mut self, data: &[u8]) -> Result<usize>;
    fn read_config(&mut self, register: u32) -> Result<u32>;
    fn write_config(&mut self, register: u32, value: u32) -> Result<()>;
    /// Tells the driver an option's been set.
    fn set_opt_callback(&mut self, option: i32, value: u32) -> Result<()>;
}

/// A device, from the device table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub idx: u32,
    pub id: u32,
    pub version: u32,
    pub read_size: u32,
    pub write_size: u32,
}

/// A frame read, borrowed from the context's buffer until the next read.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame<'a> {
    pub time: u64,
    pub dev_idx: u32,
    pub data: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Uninitialized,
    Idle,
    Running,
}

/// A controller, through its driver.
pub struct Context<D: Driver> {
    driver: D,
    devices: Vec<Device>,
    /// The largest frames, with their headers.
    max_read_frame: usize,
    max_write_frame: usize,
    block_read_size: usize,
    block_write_size: usize,
    /// Read but not yet taken as frames, from `read_pos`.
    read: Vec<u8>,
    read_pos: usize,
    state: State,
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn align(size: usize) -> usize {
    size.div_ceil(WORD) * WORD
}

/// A frame's data, padded as liboni pads it.
fn padded(size: usize) -> usize {
    size + size % WORD
}

/// Decodes a COBS packet (without its zero delimiter) in place.
fn cobs_unstuff(buffer: &mut [u8; PACKET], size: usize) {
    let (mut src, mut dst) = (0, 0);
    while src < size {
        let code = buffer[src] as usize;
        src += 1;
        let mut i = 1;
        while src < size && i < code {
            buffer[dst] = buffer[src];
            dst += 1;
            src += 1;
            i += 1;
        }
        if code < 0xff && dst < PACKET {
            buffer[dst] = 0;
            dst += 1;
        }
    }
}

impl<D: Driver> Context<D> {
    pub fn new(driver: D) -> Self {
        Context {
            driver,
            devices: Vec::new(),
            max_read_frame: 0,
            max_write_frame: 0,
            block_read_size: 0,
            block_write_size: 0,
            read: Vec::new(),
            read_pos: 0,
            state: State::Uninitialized,
        }
    }

    /// Opens the controller (`host_index` -1 for the first) and resets it.
    pub fn init(&mut self, host_index: i32) -> Result<()> {
        if self.state != State::Uninitialized {
            return Err(Error(EINVALSTATE));
        }
        self.driver.init(host_index)?;
        self.driver.write_config(CONFIG_RESET, 1)?;
        self.reset_routine()?;
        self.state = State::Idle;
        Ok(())
    }

    /// Resets the controller, which sends the device table again.
    pub fn reset(&mut self) -> Result<()> {
        if self.state != State::Idle {
            return Err(Error(EINVALSTATE));
        }
        self.driver.write_config(CONFIG_RESET, 1)?;
        self.reset_routine()?;
        self.driver.set_opt_callback(OPT_RESET, 1)
    }

    pub fn set_running(&mut self, running: bool) -> Result<()> {
        if self.state == State::Uninitialized {
            return Err(Error(EINVALSTATE));
        }
        self.driver.write_config(CONFIG_RUNNING, running as u32)?;
        // What was read is dropped: after a restart, data starts with a frame.
        self.read.clear();
        self.read_pos = 0;
        self.state = if running { State::Running } else { State::Idle };
        self.driver.set_opt_callback(OPT_RUNNING, running as u32)
    }

    /// Resets the acquisition clock (if `value` isn't 0); 2 also starts acquiring.
    pub fn reset_acq_counter(&mut self, value: u32) -> Result<()> {
        if self.state == State::Uninitialized {
            return Err(Error(EINVALSTATE));
        }
        if value != 0 {
            self.driver.write_config(CONFIG_RESETACQCOUNTER, value)?;
        }
        self.driver.set_opt_callback(OPT_RESETACQCOUNTER, value)
    }

    /// How much is read from the data stream at a time: at least the largest frame, in whole words.
    pub fn set_block_read_size(&mut self, size: u32) -> Result<()> {
        if self.state != State::Idle {
            return Err(Error(EINVALSTATE));
        }
        if (size as usize) < self.max_read_frame || !(size as usize).is_multiple_of(WORD) {
            return Err(Error(EINVALREADSIZE));
        }
        self.block_read_size = size as usize;
        self.driver.set_opt_callback(OPT_BLOCKREADSIZE, size)
    }

    pub fn read_reg(&mut self, dev_idx: u32, addr: u32) -> Result<u32> {
        self.trigger(dev_idx, addr, None)?;
        if self.pump_signal(CONFIGRACK | CONFIGRNACK)?.0 == CONFIGRNACK {
            return Err(Error(EREADFAILURE));
        }
        self.driver.read_config(CONFIG_REG_VALUE)
    }

    pub fn write_reg(&mut self, dev_idx: u32, addr: u32, value: u32) -> Result<()> {
        self.trigger(dev_idx, addr, Some(value))?;
        if self.pump_signal(CONFIGWACK | CONFIGWNACK)?.0 == CONFIGWNACK {
            return Err(Error(EWRITEFAILURE));
        }
        Ok(())
    }

    /// Sets the configuration registers for a register's read (or write, of `value`), and triggers it.
    fn trigger(&mut self, dev_idx: u32, addr: u32, value: Option<u32>) -> Result<()> {
        if self.state == State::Uninitialized {
            return Err(Error(EINVALSTATE));
        }
        if self.driver.read_config(CONFIG_TRIG)? != 0 {
            return Err(Error(ERETRIG));
        }
        self.driver.write_config(CONFIG_DEV_IDX, dev_idx)?;
        self.driver.write_config(CONFIG_REG_ADDR, addr)?;
        if let Some(value) = value {
            self.driver.write_config(CONFIG_REG_VALUE, value)?;
        }
        self.driver.write_config(CONFIG_RW, value.is_some() as u32)?;
        self.driver.write_config(CONFIG_TRIG, 1)
    }

    /// The next frame from the data stream.
    pub fn read_frame(&mut self) -> Result<Frame<'_>> {
        if self.state == State::Uninitialized || self.max_read_frame == 0 {
            return Err(Error(EINVALARG));
        }
        // Enough for the largest frame, so a frame's never split between reads.
        let remaining = self.read.len() - self.read_pos;
        if remaining < self.max_read_frame {
            self.read.copy_within(self.read_pos.., 0);
            self.read.resize(remaining + self.block_read_size, 0);
            self.read_pos = 0;
            let got = self.driver.read_stream(Stream::Data, &mut self.read[remaining..])?;
            if got != self.block_read_size {
                self.read.truncate(remaining);
                return Err(Error(EREADFAILURE));
            }
        }
        let header = self.read_pos;
        let time = u64::from_le_bytes(self.read[header..header + 8].try_into().unwrap());
        let dev_idx = word(&self.read, header + 8);
        let size = word(&self.read, header + 12) as usize;
        self.read_pos += READ_HEADER;
        if size == 0 || size > self.max_read_frame {
            return Err(Error(EBADFRAME));
        }
        let start = self.read_pos;
        self.read_pos += padded(size);
        if self.read_pos > self.read.len() {
            self.read_pos = self.read.len();
            return Err(Error(EBADFRAME));
        }
        Ok(Frame { time, dev_idx, data: &self.read[start..start + size] })
    }

    /// Writes a frame (or several, if `data`'s a multiple of the device's frames) to a device.
    pub fn write_frame(&mut self, dev_idx: u32, data: &[u8]) -> Result<()> {
        if self.state == State::Uninitialized {
            return Err(Error(EINVALARG));
        }
        let device = self.devices.iter().find(|d| d.idx == dev_idx).ok_or(Error(EDEVIDX))?;
        if device.write_size == 0 {
            return Err(Error(ENOTWRITEDEV));
        }
        if !data.len().is_multiple_of(device.write_size as usize) {
            return Err(Error(EWRITESIZE));
        }
        if WRITE_HEADER + padded(data.len()) > self.block_write_size {
            return Err(Error(EINVALARG));
        }
        let mut frame = Vec::with_capacity(WRITE_HEADER + data.len());
        frame.extend_from_slice(&dev_idx.to_le_bytes());
        frame.extend_from_slice(&((data.len() >> 2) as u32).to_le_bytes());
        frame.extend_from_slice(data);
        if self.driver.write_stream(&frame)? != frame.len() {
            return Err(Error(EWRITEFAILURE));
        }
        Ok(())
    }

    /// Takes the device table from the signal stream, after a reset.
    fn reset_routine(&mut self) -> Result<()> {
        let (_, packet) = self.pump_signal(DEVICETABLEACK)?;
        let count = word(&packet, WORD) as usize;
        let mut devices = Vec::with_capacity(count);
        for _ in 0..count {
            let (kind, packet, size) = self.read_signal()?;
            if size < WORD {
                return Err(Error(EBUFFERSIZE));
            }
            if kind != DEVICEINST {
                return Err(Error(EBADDEVTABLE));
            }
            let field = |i: usize| word(&packet, WORD * (i + 1));
            devices.push(Device {
                idx: field(0),
                id: field(1),
                version: field(2),
                read_size: field(3),
                write_size: field(4),
            });
        }
        devices.sort_by_key(|d| d.idx);
        if devices.windows(2).any(|pair| pair[0].idx == pair[1].idx) {
            return Err(Error(EDEVIDXREPEAT));
        }
        self.max_read_frame = devices.iter().map(|d| d.read_size as usize).max().unwrap_or(0) + READ_HEADER;
        self.max_write_frame = devices.iter().map(|d| d.write_size as usize).max().unwrap_or(0) + WRITE_HEADER;
        // One largest frame at a time: the least delay, until a bigger block's set.
        self.block_read_size = align(self.max_read_frame);
        self.block_write_size = align(self.max_write_frame);
        self.devices = devices;
        self.driver.set_opt_callback(OPT_BLOCKREADSIZE, self.block_read_size as u32)
    }

    /// The next packet from the signal stream, decoded, and its length (without the delimiter).
    fn read_packet(&mut self) -> Result<([u8; PACKET], usize)> {
        let mut buffer = [0; PACKET];
        let mut length = 0;
        loop {
            let mut byte = [0];
            if self.driver.read_stream(Stream::Signal, &mut byte)? != 1 {
                return Err(Error(EREADFAILURE));
            }
            if byte[0] == 0 {
                break;
            }
            if length == PACKET {
                // Too long: read to its end, then say so.
                while {
                    self.driver.read_stream(Stream::Signal, &mut byte)?;
                    byte[0] != 0
                } {}
                return Err(Error(ECOBSPACK));
            }
            buffer[length] = byte[0];
            length += 1;
        }
        cobs_unstuff(&mut buffer, length);
        Ok((buffer, length))
    }

    /// The next signal: its type, and its packet (the type, then the data) and the data's size.
    fn read_signal(&mut self) -> Result<(u32, [u8; PACKET], usize)> {
        let (packet, length) = self.read_packet()?;
        // Less COBS's overhead byte, and the type.
        let size = length.checked_sub(1 + WORD).ok_or(Error(EBUFFERSIZE))?;
        Ok((word(&packet, 0), packet, size))
    }

    /// Skips signals until one of the types in `types`, returning its type and packet.
    fn pump_signal(&mut self, types: u32) -> Result<(u32, [u8; PACKET])> {
        loop {
            match self.read_packet() {
                Ok((packet, length)) if length > 0 && word(&packet, 0) & types != 0 => {
                    return Ok((word(&packet, 0), packet));
                }
                // Malformed packets are skipped, as liboni does.
                Ok(_) | Err(Error(ECOBSPACK)) => {}
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use std::collections::{HashMap, VecDeque};

    use super::*;

    /// COBS-encodes `data`, with its delimiter.
    pub fn cobs(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0];
        let mut code_at = 0;
        for &byte in data {
            if byte == 0 {
                out[code_at] = (out.len() - code_at) as u8;
                code_at = out.len();
                out.push(0);
            } else {
                out.push(byte);
                if out.len() - code_at == 0xff {
                    out[code_at] = 0xff;
                    code_at = out.len();
                    out.push(0);
                }
            }
        }
        out[code_at] = (out.len() - code_at) as u8;
        out.push(0);
        out
    }

    pub fn signal(kind: u32, data: &[u8]) -> Vec<u8> {
        cobs(&[&kind.to_le_bytes()[..], data].concat())
    }

    /// A controller as the driver sees it: devices with registers, and frames queued to be read.
    #[derive(Default)]
    pub struct FakeController {
        pub devices: Vec<Device>,
        pub registers: HashMap<(u32, u32), u32>,
        pub config: HashMap<u32, u32>,
        pub signal: VecDeque<u8>,
        pub data: VecDeque<u8>,
        pub written: Vec<Vec<u8>>,
        pub options: Vec<(i32, u32)>,
        /// Noise before each reply, which should be skipped.
        pub noise: bool,
    }

    impl FakeController {
        pub fn with_devices(devices: &[Device]) -> Self {
            FakeController { devices: devices.to_vec(), ..Default::default() }
        }

        pub fn queue_frame(&mut self, time: u64, dev_idx: u32, data: &[u8]) {
            self.data.extend(time.to_le_bytes());
            self.data.extend(dev_idx.to_le_bytes());
            self.data.extend((data.len() as u32).to_le_bytes());
            self.data.extend(data);
            // To whole words, as controllers do.
            self.data.extend(std::iter::repeat_n(0xff, align(data.len()) - data.len()));
        }

        fn reply(&mut self, packet: Vec<u8>) {
            if self.noise {
                self.signal.extend(signal(1, &[]));
                self.signal.extend([5, 0]); // malformed: a code that runs past the packet
            }
            self.signal.extend(packet);
        }
    }

    impl Driver for FakeController {
        fn init(&mut self, _host_index: i32) -> Result<()> {
            Ok(())
        }

        fn read_stream(&mut self, stream: Stream, data: &mut [u8]) -> Result<usize> {
            let source = if stream == Stream::Data { &mut self.data } else { &mut self.signal };
            let n = data.len().min(source.len());
            for (to, from) in data.iter_mut().zip(source.drain(..n)) {
                *to = from;
            }
            Ok(n)
        }

        fn write_stream(&mut self, data: &[u8]) -> Result<usize> {
            self.written.push(data.to_vec());
            Ok(data.len())
        }

        fn read_config(&mut self, register: u32) -> Result<u32> {
            Ok(self.config.get(&register).copied().unwrap_or(0))
        }

        fn write_config(&mut self, register: u32, value: u32) -> Result<()> {
            self.config.insert(register, value);
            match (register, value) {
                (CONFIG_RESET, 1) => {
                    let table = signal(DEVICETABLEACK, &(self.devices.len() as u32).to_le_bytes());
                    self.reply(table);
                    for d in self.devices.clone() {
                        let fields = [d.idx, d.id, d.version, d.read_size, d.write_size];
                        self.signal.extend(signal(DEVICEINST, &fields.map(u32::to_le_bytes).concat()));
                    }
                }
                (CONFIG_TRIG, 1) => {
                    let key = (self.config[&CONFIG_DEV_IDX], self.config[&CONFIG_REG_ADDR]);
                    let known = self.devices.iter().any(|d| d.idx == key.0);
                    let reply = if self.config[&CONFIG_RW] == 1 {
                        self.registers.insert(key, self.config[&CONFIG_REG_VALUE]);
                        if known { CONFIGWACK } else { CONFIGWNACK }
                    } else {
                        let value = self.registers.get(&key).copied().unwrap_or(0);
                        self.config.insert(CONFIG_REG_VALUE, value);
                        if known { CONFIGRACK } else { CONFIGRNACK }
                    };
                    self.reply(signal(reply, &[]));
                    self.config.insert(CONFIG_TRIG, 0);
                }
                _ => {}
            }
            Ok(())
        }

        fn set_opt_callback(&mut self, option: i32, value: u32) -> Result<()> {
            self.options.push((option, value));
            Ok(())
        }
    }

    const RHYTHM: Device = Device { idx: 0x0101, id: 7, version: 1, read_size: 30, write_size: 0 };
    const TTL: Device = Device { idx: 0x0102, id: 8, version: 1, read_size: 8, write_size: 4 };

    fn opened(devices: &[Device]) -> Context<FakeController> {
        let mut context = Context::new(FakeController::with_devices(devices));
        context.init(-1).unwrap();
        context
    }

    #[test]
    fn cobs_round_trips() {
        for data in [vec![1, 2, 3], vec![0], vec![0, 0, 5, 0], vec![], (1..=254).collect(), vec![7; 300]] {
            let encoded = cobs(&data);
            let length = encoded.len() - 1;
            if length > PACKET {
                continue;
            }
            assert!(!encoded[..length].contains(&0));
            let mut buffer = [0; PACKET];
            buffer[..length].copy_from_slice(&encoded[..length]);
            cobs_unstuff(&mut buffer, length);
            assert_eq!(&buffer[..data.len()], &data[..], "{data:?}");
        }
    }

    #[test]
    fn init_takes_the_device_table_sorted() {
        let context = opened(&[TTL, RHYTHM]);
        assert_eq!(context.devices, [RHYTHM, TTL]);
        assert_eq!(context.max_read_frame, 30 + READ_HEADER);
        assert_eq!(context.block_read_size, 48);
        assert_eq!(context.driver.options, vec![(OPT_BLOCKREADSIZE, 48)]);
    }

    #[test]
    fn a_repeated_device_is_an_error() {
        let mut context = Context::new(FakeController::with_devices(&[RHYTHM, RHYTHM]));
        assert_eq!(context.init(-1), Err(Error(EDEVIDXREPEAT)));
    }

    #[test]
    fn registers_are_written_and_read_back() {
        let mut context = opened(&[RHYTHM]);
        context.driver.noise = true;
        context.write_reg(0x0101, 5, 0xdead).unwrap();
        assert_eq!(context.driver.registers[&(0x0101, 5)], 0xdead);
        assert_eq!(context.read_reg(0x0101, 5), Ok(0xdead));
        assert_eq!(context.read_reg(0x0101, 6), Ok(0));
        assert_eq!(context.driver.signal.len(), 0);
    }

    #[test]
    fn a_refused_register_is_an_error() {
        let mut context = opened(&[RHYTHM]);
        assert_eq!(context.write_reg(0x0999, 5, 1), Err(Error(EWRITEFAILURE)));
        assert_eq!(context.read_reg(0x0999, 5), Err(Error(EREADFAILURE)));
    }

    #[test]
    fn a_register_isnt_triggered_twice() {
        let mut context = opened(&[RHYTHM]);
        context.driver.config.insert(CONFIG_TRIG, 1);
        assert_eq!(context.read_reg(0x0101, 5), Err(Error(ERETRIG)));
    }

    #[test]
    fn frames_are_read_across_blocks() {
        let mut context = opened(&[RHYTHM, TTL]);
        context.set_block_read_size(64).unwrap();
        let frames: Vec<(u64, u32, Vec<u8>)> = (0..20)
            .map(|i| if i % 3 == 0 { (i, 0x0102, vec![i as u8; 8]) } else { (i, 0x0101, vec![i as u8; 30]) })
            .collect();
        for (time, dev, data) in &frames {
            context.driver.queue_frame(*time, *dev, data);
        }
        // Whole blocks only.
        let whole = context.driver.data.len() / 64 * 64;
        context.driver.data.truncate(whole);
        let mut read = Vec::new();
        while let Ok(frame) = context.read_frame() {
            read.push((frame.time, frame.dev_idx, frame.data.to_vec()));
        }
        assert!(read.len() >= 15, "{}", read.len());
        assert_eq!(read[..], frames[..read.len()]);
    }

    #[test]
    fn a_short_read_is_an_error() {
        let mut context = opened(&[RHYTHM]);
        context.driver.queue_frame(1, 0x0101, &[1; 30]);
        context.driver.data.truncate(40);
        assert_eq!(context.read_frame(), Err(Error(EREADFAILURE)));
    }

    #[test]
    fn a_frame_too_big_is_an_error() {
        let mut context = opened(&[RHYTHM]);
        context.set_block_read_size(128).unwrap();
        // Bigger than the largest frame with its header (as liboni checks).
        context.driver.queue_frame(1, 0x0101, &[1; 47]);
        context.driver.queue_frame(2, 0x0101, &[2; 30]);
        context.driver.queue_frame(3, 0x0101, &[3; 30]);
        assert_eq!(context.read_frame(), Err(Error(EBADFRAME)));
    }

    #[test]
    fn running_drops_what_was_read() {
        let mut context = opened(&[TTL]);
        // Two frames a block.
        context.set_block_read_size(48).unwrap();
        for i in 0..4 {
            context.driver.queue_frame(i, 0x0102, &[i as u8; 8]);
        }
        assert_eq!(context.read_frame().unwrap().time, 0);
        context.set_running(true).unwrap();
        // Frame 1, in the first block, is gone.
        assert_eq!(context.read_frame().unwrap().time, 2);
    }

    #[test]
    fn the_block_read_size_holds_the_largest_frame() {
        let mut context = opened(&[RHYTHM]);
        assert_eq!(context.set_block_read_size(44), Err(Error(EINVALREADSIZE)));
        assert_eq!(context.set_block_read_size(50), Err(Error(EINVALREADSIZE)));
        context.set_block_read_size(24 * 1024).unwrap();
        context.set_running(true).unwrap();
        assert_eq!(context.set_block_read_size(1024), Err(Error(EINVALSTATE)));
    }

    #[test]
    fn frames_are_written_with_their_header() {
        let mut context = opened(&[RHYTHM, TTL]);
        context.write_frame(0x0102, &[1, 2, 3, 4]).unwrap();
        assert_eq!(context.driver.written, vec![vec![2, 1, 0, 0, 1, 0, 0, 0, 1, 2, 3, 4]]);
        assert_eq!(context.write_frame(0x0101, &[1, 2, 3, 4]), Err(Error(ENOTWRITEDEV)));
        assert_eq!(context.write_frame(0x0102, &[1, 2]), Err(Error(EWRITESIZE)));
        assert_eq!(context.write_frame(0x0999, &[1, 2, 3, 4]), Err(Error(EDEVIDX)));
    }

    #[test]
    fn reset_and_counter_tell_the_driver() {
        let mut context = opened(&[RHYTHM]);
        context.reset().unwrap();
        context.reset_acq_counter(2).unwrap();
        assert_eq!(context.driver.config[&CONFIG_RESETACQCOUNTER], 2);
        assert_eq!(
            context.driver.options,
            vec![(OPT_BLOCKREADSIZE, 48), (OPT_BLOCKREADSIZE, 48), (OPT_RESET, 1), (OPT_RESETACQCOUNTER, 2)]
        );
    }
}
