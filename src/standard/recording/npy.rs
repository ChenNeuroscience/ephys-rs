//! NumPy `.npy` files of one dimension, written a value at a time, with their length filled in at the end.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// The header's size: room for any length.
const HEADER: usize = 128;

pub struct Npy {
    file: BufWriter<File>,
    /// The values' type, e.g. "<i8" (little-endian 8-byte integers).
    descr: &'static str,
    len: u64,
}

impl Npy {
    pub fn create(path: &Path, descr: &'static str) -> io::Result<Self> {
        let mut file = BufWriter::new(File::create(path)?);
        file.write_all(&header(descr, 0))?;
        Ok(Npy { file, descr, len: 0 })
    }

    /// Writes values already in the file's type (`descr`), as little-endian bytes.
    pub fn write(&mut self, bytes: &[u8], count: usize) -> io::Result<()> {
        self.len += count as u64;
        self.file.write_all(bytes)
    }

    pub fn write_i64(&mut self, value: i64) -> io::Result<()> {
        self.write(&value.to_le_bytes(), 1)
    }

    pub fn write_f64(&mut self, value: f64) -> io::Result<()> {
        self.write(&value.to_le_bytes(), 1)
    }

    /// Fills in the length, and closes the file.
    pub fn finish(mut self) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header(self.descr, self.len))?;
        self.file.flush()
    }
}

/// The magic string, version 1.0, then the header dictionary, padded with spaces to `HEADER` bytes.
fn header(descr: &str, len: u64) -> Vec<u8> {
    let dict = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': ({len},), }}");
    let mut bytes = b"\x93NUMPY\x01\x00".to_vec();
    bytes.extend_from_slice(&((HEADER - 10) as u16).to_le_bytes());
    bytes.extend_from_slice(dict.as_bytes());
    bytes.resize(HEADER - 1, b' ');
    bytes.push(b'\n');
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_numpy_file() {
        let path = std::env::temp_dir().join(format!("ephys-rs-test-{}.npy", std::process::id()));
        let mut npy = Npy::create(&path, "<i8").unwrap();
        for v in [3, -1, 7] {
            npy.write_i64(v).unwrap();
        }
        npy.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(bytes.len(), HEADER + 24);
        let text = String::from_utf8_lossy(&bytes[10..HEADER]);
        assert!(text.contains("'shape': (3,)"), "{text}");
        assert_eq!(i64::from_le_bytes(bytes[HEADER + 8..HEADER + 16].try_into().unwrap()), -1);
    }
}
