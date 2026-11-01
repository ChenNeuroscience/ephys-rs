//! The data edge types: what each kind of signal looks like, as known when configuring the graph.

use eframe::egui::Color32;

use crate::engine::{EdgeInfo, EdgeType, SignalType};
use crate::tracking::pipeline::Blob;

/// Regularly sampled voltages.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Continuous {
    pub channels: u32,
    pub sample_rate: f32,
}

impl EdgeType for Continuous {
    const INFO: EdgeInfo = EdgeInfo {
        name: "continuous",
        colour: Color32::from_rgb(90, 160, 255),
        description: "Regularly sampled voltages, one or more channels.",
    };

    fn summary(&self) -> String {
        format!("{} ch · {}", self.channels, format_rate(self.sample_rate))
    }
}

/// TTL-style digital events.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Events {
    pub lines: u32,
}

impl EdgeType for Events {
    const INFO: EdgeInfo = EdgeInfo {
        name: "events",
        colour: Color32::from_rgb(190, 130, 255),
        description: "Digital events, e.g. TTL inputs.",
    };

    fn summary(&self) -> String {
        format!("{} lines", self.lines)
    }
}

/// Detected spikes: a timestamp, channel and waveform each.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Spikes {
    pub channels: u32,
    pub sample_rate: f32,
}

impl EdgeType for Spikes {
    const INFO: EdgeInfo = EdgeInfo {
        name: "spikes",
        colour: Color32::from_rgb(255, 160, 60),
        description: "Detected spikes, each with a timestamp, channel and waveform.",
    };

    fn summary(&self) -> String {
        format!("{} ch · {}", self.channels, format_rate(self.sample_rate))
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Video {
    pub width: u32,
    pub height: u32,
    pub fps: f32,
}

impl EdgeType for Video {
    const INFO: EdgeInfo =
        EdgeInfo { name: "video", colour: Color32::from_rgb(90, 210, 130), description: "Video frames." };

    fn summary(&self) -> String {
        format!("{}×{} · {} fps", self.width, self.height, self.fps)
    }
}

/// The positions of LEDs tracked in video: for each frame, the biggest two found.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Positions {
    /// Frames per second.
    pub fps: f32,
}

impl EdgeType for Positions {
    const INFO: EdgeInfo = EdgeInfo {
        name: "positions",
        colour: Color32::from_rgb(240, 100, 160),
        description: "LED positions tracked in video, frame by frame.",
    };

    fn summary(&self) -> String {
        format!("{} fps", self.fps)
    }
}

/// Where the LEDs were in one frame.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // the time and calibration are for writers
pub struct PositionSample {
    /// The camera's frame id and timestamp (seconds, in the camera's clock).
    pub frame: u64,
    pub time: f64,
    /// The biggest LED found, then the second biggest, in pixels.
    pub leds: [Option<Blob>; 2],
    /// The calibration when it was found; NaN if there wasn't one.
    pub pixels_per_metre: f32,
}

pub struct PositionsBuffer {
    pub samples: Vec<PositionSample>,
}

impl SignalType for Positions {
    type Buffer = PositionsBuffer;

    fn buffer(&self, duration: f64) -> PositionsBuffer {
        PositionsBuffer { samples: Vec::with_capacity(per_pass(self.fps, duration) + 1) }
    }

    fn bytes(buffer: &PositionsBuffer) -> usize {
        buffer.samples.capacity() * size_of::<PositionSample>()
    }
}

/// The most samples (or frames) of a signal at `rate` that fall in `duration` seconds.
fn per_pass(rate: f32, duration: f64) -> usize {
    (rate as f64 * duration).ceil() as usize + 1
}

/// A pass's worth of continuous data: for each channel, a row of up to `capacity` samples, of which the first
/// `len` are this pass's.
pub struct ContinuousBuffer {
    pub channels: usize,
    pub capacity: usize,
    pub len: usize,
    /// Channel after channel, each `capacity` long.
    pub data: Vec<f32>,
}

impl ContinuousBuffer {
    pub fn channel(&self, channel: usize) -> &[f32] {
        let start = channel * self.capacity;
        &self.data[start..start + self.len]
    }

    pub fn channel_mut(&mut self, channel: usize) -> &mut [f32] {
        let start = channel * self.capacity;
        &mut self.data[start..start + self.len]
    }

    /// Fills this buffer sample by sample, with `f(channel, sample)` of each sample of `input`, or (when this
    /// buffer is being made in place, and `input` is `None`) of its own samples.
    pub fn transform(&mut self, input: Option<&ContinuousBuffer>, mut f: impl FnMut(usize, f32) -> f32) {
        if let Some(input) = input {
            self.len = input.len.min(self.capacity);
        }
        for channel in 0..self.channels {
            let start = channel * self.capacity;
            let output = &mut self.data[start..start + self.len];
            match input {
                Some(input) => {
                    for (y, x) in output.iter_mut().zip(input.channel(channel)) {
                        *y = f(channel, *x);
                    }
                }
                None => {
                    for y in output {
                        *y = f(channel, *y);
                    }
                }
            }
        }
    }
}

impl SignalType for Continuous {
    type Buffer = ContinuousBuffer;

    fn buffer(&self, duration: f64) -> ContinuousBuffer {
        let (channels, capacity) = (self.channels as usize, per_pass(self.sample_rate, duration));
        ContinuousBuffer { channels, capacity, len: 0, data: vec![0.0; channels * capacity] }
    }

    fn bytes(buffer: &ContinuousBuffer) -> usize {
        buffer.data.capacity() * size_of::<f32>()
    }
}

/// A digital event: a line going high or low, at a sample within the pass.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // read by viewers and writers, once they're fed real data
pub struct Event {
    pub sample: u32,
    pub line: u8,
    pub high: bool,
}

pub struct EventsBuffer {
    pub events: Vec<Event>,
}

impl SignalType for Events {
    type Buffer = EventsBuffer;

    fn buffer(&self, _duration: f64) -> EventsBuffer {
        EventsBuffer { events: Vec::with_capacity(64 * self.lines.max(1) as usize) }
    }

    fn bytes(buffer: &EventsBuffer) -> usize {
        buffer.events.capacity() * size_of::<Event>()
    }
}

/// A detected spike, at a sample within the pass.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // read by viewers and writers, once they're fed real data
pub struct Spike {
    pub sample: u32,
    pub channel: u32,
}

pub struct SpikesBuffer {
    /// Room for up to one spike per millisecond per channel; any beyond that are lost.
    pub spikes: Vec<Spike>,
}

impl SignalType for Spikes {
    type Buffer = SpikesBuffer;

    fn buffer(&self, duration: f64) -> SpikesBuffer {
        let capacity = self.channels as usize * per_pass(1000.0, duration);
        SpikesBuffer { spikes: Vec::with_capacity(capacity) }
    }

    fn bytes(buffer: &SpikesBuffer) -> usize {
        buffer.spikes.capacity() * size_of::<Spike>()
    }
}

/// A pass's video frames (often none or one), each `width` × `height` bytes of grey.
pub struct VideoBuffer {
    pub width: usize,
    pub height: usize,
    pub frames: usize,
    pub capacity: usize,
    pub pixels: Vec<u8>,
    /// For each frame, the camera's frame id and timestamp (seconds, in the camera's clock).
    pub ids: Vec<u64>,
    pub times: Vec<f64>,
}

impl VideoBuffer {
    pub fn frame(&self, frame: usize) -> &[u8] {
        let size = self.width * self.height;
        &self.pixels[frame * size..(frame + 1) * size]
    }

    pub fn frame_mut(&mut self, frame: usize) -> &mut [u8] {
        let size = self.width * self.height;
        &mut self.pixels[frame * size..(frame + 1) * size]
    }
}

impl SignalType for Video {
    type Buffer = VideoBuffer;

    fn buffer(&self, duration: f64) -> VideoBuffer {
        // A frame more than strictly needed, as cameras' timing jitters.
        let (width, height, capacity) = (self.width as usize, self.height as usize, per_pass(self.fps, duration) + 1);
        VideoBuffer {
            width,
            height,
            frames: 0,
            capacity,
            pixels: vec![0; width * height * capacity],
            ids: vec![0; capacity],
            times: vec![0.0; capacity],
        }
    }

    fn bytes(buffer: &VideoBuffer) -> usize {
        buffer.pixels.capacity()
    }
}

/// "30 kHz", "2.5 kHz", "500 Hz".
pub fn format_rate(hz: f32) -> String {
    if hz >= 1000.0 { format!("{} kHz", hz / 1000.0) } else { format!("{hz} Hz") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries() {
        assert_eq!(Continuous { channels: 32, sample_rate: 30000.0 }.summary(), "32 ch · 30 kHz");
        assert_eq!(Spikes { channels: 4, sample_rate: 2500.0 }.summary(), "4 ch · 2.5 kHz");
        assert_eq!(Video { width: 640, height: 480, fps: 30.0 }.summary(), "640×480 · 30 fps");
    }
}
