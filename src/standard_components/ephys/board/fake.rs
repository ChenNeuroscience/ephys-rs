//! Client for `synthetic_streams/fake_acquisition_board.py`. On connecting, the board sends a line of JSON
//! describing itself:
//!
//! ```text
//! {"description": "...", "headstages": [{"port": "A1", "channels": 32}, ...], "adc_channels": 8, "ttl_lines": 8}
//! ```
//!
//! We reply with a line of JSON saying how to run, `{"sample_rate": 30000}`, and it streams blocks of samples,
//! each a 28 byte little-endian header (`b"RHD1"`, u32 samples, u32 headstage channels, u32 auxiliary channels,
//! u32 analogue channels, u64 first sample number) then, as i16, each headstage channel's samples in turn
//! (0.195 µV per bit), then each auxiliary channel's (37.4 µV per bit) and each analogue channel's (152.6 µV
//! per bit), then as u16 the digital inputs for each sample (a bit per line).

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{
    ADC_VOLTS_PER_BIT, AUX_VOLTS_PER_BIT, Block, Board, BoardInfo, Config, EPHYS_MICROVOLTS_PER_BIT, Headstage,
};

const MAGIC: &[u8; 4] = b"RHD1";
const HEADER_LEN: usize = 28;
const MAX_VALUES: usize = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Description {
    description: String,
    headstages: Vec<Headstage>,
    adc_channels: u32,
    ttl_lines: u32,
}

#[derive(Serialize, Deserialize)]
struct Start {
    sample_rate: u32,
}

pub struct FakeBoard {
    stream: TcpStream,
    info: BoardInfo,
    /// Partially received block, kept across read timeouts so no bytes are lost.
    buf: Vec<u8>,
    filled: usize,
}

impl FakeBoard {
    pub fn connect(address: &str) -> io::Result<Self> {
        let addr = address
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address"))?;
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(1))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        let line = read_line(&mut stream, Duration::from_secs(5))?;
        let d: Description = serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let info = BoardInfo {
            description: d.description,
            headstages: d.headstages,
            adc_channels: d.adc_channels,
            ttl_lines: d.ttl_lines,
        };
        Ok(FakeBoard { stream, info, buf: vec![0; HEADER_LEN], filled: 0 })
    }

    /// Reads until `buf` is full. Returns false on timeout (progress is kept).
    fn fill(&mut self) -> io::Result<bool> {
        while self.filled < self.buf.len() {
            match self.stream.read(&mut self.buf[self.filled..]) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.filled += n,
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => return Ok(false),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }
}

/// Reads a line (without its newline), giving up after `timeout`.
fn read_line(stream: &mut TcpStream, timeout: Duration) -> io::Result<String> {
    let deadline = Instant::now() + timeout;
    let mut line = Vec::new();
    let mut byte = [0u8];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => line.push(byte[0]),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
        if Instant::now() > deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "no description from the board"));
        }
        if line.len() > 1 << 20 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "description too long"));
        }
    }
    String::from_utf8(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

impl Board for FakeBoard {
    fn info(&self) -> &BoardInfo {
        &self.info
    }

    fn start(&mut self, config: &Config) -> io::Result<()> {
        let mut line = serde_json::to_vec(&Start { sample_rate: config.sample_rate })?;
        line.push(b'\n');
        self.stream.write_all(&line)
    }

    fn read(&mut self, block: &mut Block) -> io::Result<bool> {
        if self.buf.len() == HEADER_LEN {
            if !self.fill()? {
                return Ok(false);
            }
            if &self.buf[0..4] != MAGIC {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "bad block magic"));
            }
            let [samples, ephys, aux, adc] = [4, 8, 12, 16].map(|at| u32_at(&self.buf, at) as usize);
            let values = samples * (ephys + aux + adc + 1);
            if samples == 0 || values > MAX_VALUES {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("bad block of {samples} samples")));
            }
            self.buf.resize(HEADER_LEN + 2 * values, 0);
        }
        if !self.fill()? {
            return Ok(false);
        }

        let [samples, ephys, aux, adc] = [4, 8, 12, 16].map(|at| u32_at(&self.buf, at) as usize);
        block.resize(samples, ephys, aux, adc);
        block.first = u64::from_le_bytes(self.buf[20..28].try_into().unwrap());
        let mut values = self.buf[HEADER_LEN..].chunks_exact(2).map(|b| [b[0], b[1]]);
        let mut scale = |out: &mut [f32], per_bit: f32| {
            for (value, bytes) in out.iter_mut().zip(values.by_ref()) {
                *value = i16::from_le_bytes(bytes) as f32 * per_bit;
            }
        };
        scale(&mut block.ephys, EPHYS_MICROVOLTS_PER_BIT);
        scale(&mut block.aux, AUX_VOLTS_PER_BIT);
        scale(&mut block.adc, ADC_VOLTS_PER_BIT);
        for (word, bytes) in block.ttl.iter_mut().zip(values) {
            *word = u16::from_le_bytes(bytes) as u32;
        }
        self.buf.truncate(HEADER_LEN);
        self.filled = 0;
        Ok(true)
    }
}

fn u32_at(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}

/// Serves a board the way `fake_acquisition_board.py` does, for tests that can't rely on Python: headstages
/// A1 and B1 of 32 channels, 8 analogue inputs and 8 digital lines. Each headstage channel has a slow wave,
/// noise, and a spike now and then; the first digital line goes high for 10 ms every half second. Blocks of
/// 10 ms go out in real time until the client goes. Returns the address.
#[cfg(test)]
pub fn serve_test_board() -> String {
    use std::net::TcpListener;

    use crate::standard_components::preview::{noise, spike_shape};

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let headstages =
            vec![Headstage { port: "A1".into(), channels: 32 }, Headstage { port: "B1".into(), channels: 32 }];
        let description = Description {
            description: "Test board".into(),
            headstages: headstages.clone(),
            adc_channels: 8,
            ttl_lines: 8,
        };
        let mut line = serde_json::to_vec(&description).unwrap();
        line.push(b'\n');
        if stream.write_all(&line).is_err() {
            return;
        }
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let Ok(start) = read_line(&mut stream, Duration::from_secs(5)) else { return };
        let Ok(Start { sample_rate }) = serde_json::from_str(&start) else { return };
        let (ephys, aux, adc) = (Headstage::total(&headstages) as usize, 3 * headstages.len(), 8);
        let samples = (sample_rate / 100) as usize;
        let ms = (sample_rate / 1000).max(1) as u64;
        let begin = Instant::now();
        for block in 0u64.. {
            let first = block * samples as u64;
            let mut out = Vec::with_capacity(HEADER_LEN + 2 * samples * (ephys + aux + adc + 1));
            out.extend_from_slice(MAGIC);
            for n in [samples, ephys, aux, adc] {
                out.extend_from_slice(&(n as u32).to_le_bytes());
            }
            out.extend_from_slice(&first.to_le_bytes());
            let microvolts = |v: f32| ((v / EPHYS_MICROVOLTS_PER_BIT) as i16).to_le_bytes();
            for channel in 0..ephys as u64 {
                for n in first..first + samples as u64 {
                    let wave = 50.0 * (std::f64::consts::TAU * 8.0 * n as f64 / sample_rate as f64).sin() as f32;
                    let spike = if noise(channel, (n / ms) as i64) > 0.99 {
                        -150.0 * spike_shape((n % ms) as f32 / ms as f32)
                    } else {
                        0.0
                    };
                    out.extend_from_slice(&microvolts(wave + 20.0 * noise(1000 + channel, n as i64) + spike));
                }
            }
            out.resize(out.len() + 2 * samples * (aux + adc), 0);
            for n in first..first + samples as u64 {
                let high = n % (sample_rate as u64 / 2) < 10 * ms;
                out.extend_from_slice(&(high as u16).to_le_bytes());
            }
            if stream.write_all(&out).is_err() {
                return;
            }
            let due = begin + Duration::from_millis(10 * (block + 1));
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
    });
    address
}
