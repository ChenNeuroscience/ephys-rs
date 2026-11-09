//! Client for `synthetic_streams/fake_camera.py`. Each frame on the wire is a 28 byte little-endian header
//! (`b"FRM1"`, u32 width, u32 height, u64 frame id, f64 timestamp seconds) then width*height gray8 bytes.

use std::io::{self, Read};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use super::{Camera, FrameInfo};

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
        "Fake camera (synthetic_streams/fake_camera.py)".into()
    }

    fn fetch(&mut self) -> io::Result<Option<FrameInfo>> {
        if self.buf.len() > HEADER_LEN && self.filled == self.buf.len() {
            // A whole frame, not copied out yet.
        } else {
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
        }
        let (width, height) = (u32_at(&self.buf, 4), u32_at(&self.buf, 8));
        let id = u64::from_le_bytes(self.buf[12..20].try_into().unwrap());
        let timestamp = f64::from_le_bytes(self.buf[20..28].try_into().unwrap());
        Ok(Some(FrameInfo { id, timestamp, width, height }))
    }

    fn copy_to(&mut self, into: Option<&mut [u8]>) {
        if let Some(into) = into {
            into.copy_from_slice(&self.buf[HEADER_LEN..]);
        }
        self.buf.truncate(HEADER_LEN);
        self.filled = 0;
    }
}

fn u32_at(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}

/// Serves a test scene the way `fake_camera.py` does, for tests that can't rely on Python: two LEDs (one at 255
/// and a smaller one at 240) on a dim background, at 50 fps, until the client goes. Returns the address.
#[cfg(test)]
pub fn serve_test_scene(width: u32, height: u32) -> String {
    use std::io::Write;
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let mut pixels = vec![20u8; (width * height) as usize];
        let mut led = |x: u32, y: u32, radius: u32, value: u8| {
            for dy in 0..=2 * radius {
                for dx in 0..=2 * radius {
                    let (px, py) = (x + dx - radius, y + dy - radius);
                    if dx.abs_diff(radius).pow(2) + dy.abs_diff(radius).pow(2) <= radius * radius {
                        pixels[(py * width + px) as usize] = value;
                    }
                }
            }
        };
        led(width / 2, height / 2, 5, 255);
        led(width / 2 + 25, height / 2, 3, 240);
        for id in 0u64.. {
            let mut frame = Vec::with_capacity(HEADER_LEN + pixels.len());
            frame.extend_from_slice(MAGIC);
            frame.extend_from_slice(&width.to_le_bytes());
            frame.extend_from_slice(&height.to_le_bytes());
            frame.extend_from_slice(&id.to_le_bytes());
            frame.extend_from_slice(&(id as f64 / 50.0).to_le_bytes());
            frame.extend_from_slice(&pixels);
            if stream.write_all(&frame).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    address
}
