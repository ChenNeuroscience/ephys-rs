//! Log lines: lines of text as a data stream, each of at most `LogLines::width` bytes, and each with a time, in seconds
//! by its source's own clock (as positions and events have). They're held fixed width, so a
//! pass's buffer is made once, with room for `LINES_PER_PASS` lines, and nothing's allocated as they're passed on. The
//! Log Viewer shows them (`log_viewer`).

pub mod log_viewer;

use eframe::egui::Color32;

use crate::engine::{DataStreamType, EdgeInfo, EdgeType};

/// Room in a pass's buffer: 3,200 lines a second. A source with more keeps them for the next pass.
pub const LINES_PER_PASS: usize = 64;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LogLines {
    /// The most bytes in a line: longer ones are cut short.
    pub width: u32,
}

impl EdgeType for LogLines {
    const INFO: EdgeInfo = EdgeInfo {
        name: "log lines",
        colour: Color32::from_rgb(215, 195, 120),
        description: "Lines of text, each up to a maximum number of bytes.",
    };

    fn summary(&self) -> String {
        format!("up to {} bytes a line", self.width)
    }
}

/// A pass's lines: line `i` is at `i * width` in `bytes`, `lengths[i]` long, from `times[i]`.
pub struct LogLinesBuffer {
    width: usize,
    bytes: Vec<u8>,
    lengths: Vec<u32>,
    times: Vec<f64>,
}

impl LogLinesBuffer {
    pub fn clear(&mut self) {
        self.lengths.clear();
        self.times.clear();
    }

    pub fn is_full(&self) -> bool {
        self.lengths.len() == LINES_PER_PASS
    }

    /// Adds a line from `time`, if there's room: cut short to fit (keeping whole characters), and kept to one line.
    /// Returns whether there was.
    pub fn push(&mut self, time: f64, line: &str) -> bool {
        if self.is_full() {
            return false;
        }
        let mut length = line.len().min(self.width);
        while !line.is_char_boundary(length) {
            length -= 1;
        }
        let start = self.lengths.len() * self.width;
        let kept = &mut self.bytes[start..start + length];
        kept.copy_from_slice(&line.as_bytes()[..length]);
        for byte in kept.iter_mut().filter(|byte| matches!(byte, b'\r' | b'\n')) {
            *byte = b' ';
        }
        self.lengths.push(length as u32);
        self.times.push(time);
        true
    }

    /// The lines with their times, oldest first.
    pub fn lines(&self) -> impl Iterator<Item = (f64, &[u8])> {
        let lines = self.lengths.iter().enumerate();
        lines.map(|(i, &length)| (self.times[i], &self.bytes[i * self.width..][..length as usize]))
    }
}

impl DataStreamType for LogLines {
    type Buffer = LogLinesBuffer;

    fn buffer(&self, _duration: f64) -> LogLinesBuffer {
        let width = self.width as usize;
        LogLinesBuffer {
            width,
            bytes: vec![0; width * LINES_PER_PASS],
            lengths: Vec::with_capacity(LINES_PER_PASS),
            times: Vec::with_capacity(LINES_PER_PASS),
        }
    }

    fn bytes(buffer: &LogLinesBuffer) -> usize {
        buffer.bytes.capacity()
            + buffer.lengths.capacity() * size_of::<u32>()
            + buffer.times.capacity() * size_of::<f64>()
    }

    fn copy(from: &LogLinesBuffer, to: &mut LogLinesBuffer) {
        let used = from.lengths.len() * from.width;
        to.bytes[..used].copy_from_slice(&from.bytes[..used]);
        to.lengths.clone_from(&from.lengths);
        to.times.clone_from(&from.times);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::PASS_DURATION;

    #[test]
    fn long_lines_are_cut_short_keeping_whole_characters_and_stay_one_line() {
        let mut buffer = LogLines { width: 5 }.buffer(PASS_DURATION);
        assert!(buffer.push(1.0, "hello there"));
        assert!(buffer.push(2.0, "abcdé!"), "é is two bytes, the fifth and sixth");
        assert!(buffer.push(3.0, "a\r\nb"));
        let lines: Vec<(f64, &[u8])> = buffer.lines().collect();
        assert_eq!(lines, [(1.0, &b"hello"[..]), (2.0, b"abcd"), (3.0, b"a  b")]);
    }

    #[test]
    fn a_full_buffer_says_so_and_copies_whole() {
        let stream = LogLines { width: 8 };
        let (mut from, mut to) = (stream.buffer(PASS_DURATION), stream.buffer(PASS_DURATION));
        for _ in 0..LINES_PER_PASS {
            assert!(from.push(7.0, "line"));
        }
        assert!(!from.push(7.0, "no room"));
        LogLines::copy(&from, &mut to);
        assert_eq!(to.lines().count(), LINES_PER_PASS);
        assert!(to.lines().all(|line| line == (7.0, &b"line"[..])));
    }
}
