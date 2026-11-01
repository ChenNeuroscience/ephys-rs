//! Client for `tools/fake_camera.py`. Each frame on the wire is a 28 byte little-endian header
//! (`b"FRM1"`, u32 width, u32 height, u64 frame id, f64 timestamp seconds) then width*height gray8 bytes.

use std::io::{self, Read};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use super::{Camera, Frame};

const MAGIC: &[u8; 4] = b"FRM1";
const HEADER_LEN: usize = 28;
const MAX_PIXELS: usize = 8192 * 8192;

pub struct FakeCamera {
    stream: TcpStream,
    /// Partially received frame, kept across read timeouts so no bytes are lost.
    buf: Vec<u8>,
    filled: usize,
}

impl FakeCamera {
    pub fn connect(address: &str) -> io::Result<Self> {
        let addr = address
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address"))?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(1))?;
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        stream.set_nodelay(true)?;
        Ok(FakeCamera { stream, buf: vec![0; HEADER_LEN], filled: 0 })
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

impl Camera for FakeCamera {
    fn description(&self) -> String {
        "Fake camera (tools/fake_camera.py)".into()
    }

    fn next_frame(&mut self) -> io::Result<Option<Frame>> {
        if self.buf.len() == HEADER_LEN {
            if !self.fill()? {
                return Ok(None);
            }
            if &self.buf[0..4] != MAGIC {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "bad frame magic"));
            }
            let (w, h) = (u32_at(&self.buf, 4) as usize, u32_at(&self.buf, 8) as usize);
            if w == 0 || h == 0 || w * h > MAX_PIXELS {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("bad frame size {w}x{h}")));
            }
            self.buf.resize(HEADER_LEN + w * h, 0);
        }
        if !self.fill()? {
            return Ok(None);
        }

        let (w, h) = (u32_at(&self.buf, 4), u32_at(&self.buf, 8));
        let id = u64::from_le_bytes(self.buf[12..20].try_into().unwrap());
        let timestamp = f64::from_le_bytes(self.buf[20..28].try_into().unwrap());
        let pixels = self.buf.split_off(HEADER_LEN);
        self.filled = 0;
        Ok(Some(Frame { id, timestamp, width: w, height: h, pixels }))
    }
}

fn u32_at(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}
