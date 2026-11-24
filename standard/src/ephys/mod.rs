//! Electrophysiology: continuous voltages and digital events, from an acquisition board, re-referenced,
//! filtered, downsampled, scaled and viewed.

pub mod band_pass;
pub mod board;
pub mod channel_selector;
pub mod continuous_gain;
pub mod continuous_viewer;
pub mod downsample;
pub mod hilbert;
pub mod phase_viewer;
pub mod reference;
pub mod spectrum;
pub mod ttl_viewer;

use eframe::egui::Color32;

use super::clock::{Clock, ClockId};
use super::per_pass;
use super::widgets::format_rate;
use ephys_rs::engine::{DataStreamType, EdgeInfo, EdgeType};

/// Regularly sampled voltages.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Continuous {
    pub channels: u32,
    pub sample_rate: f32,
    /// The clock that times the samples, counted in samples (see `crate::standard::clock`).
    pub clock: ClockId,
}

impl Continuous {
    /// The clock that times the samples, with the sample rate as its timebase, as `ContinuousBuffer::start` counts.
    pub fn timing(&self) -> Clock {
        Clock::new(self.clock, self.sample_rate as f64)
    }
}

impl EdgeType for Continuous {
    const INFO: EdgeInfo = EdgeInfo {
        name: "continuous",
        colour: Color32::from_rgb(90, 160, 255),
        description: "Regularly sampled voltages, on any number of channels, even none.",
    };

    fn summary(&self) -> String {
        format!("{} ch · {}", self.channels, format_rate(self.sample_rate))
    }
}

/// TTL-style digital events.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Events {
    pub lines: u32,
    /// The clock that times the events (see `Event::at`): counting a board's samples, say, or a camera's seconds.
    pub clock: Clock,
}

impl EdgeType for Events {
    const INFO: EdgeInfo = EdgeInfo {
        name: "events",
        colour: Color32::from_rgb(190, 130, 255),
        description: "Digital events, such as TTL inputs.",
    };

    fn summary(&self) -> String {
        format!("{} line{}", self.lines, if self.lines == 1 { "" } else { "s" })
    }
}

/// A pass's worth of continuous data: for each channel, a row of up to `capacity` samples, of which the first
/// `len` are this pass's.
pub struct ContinuousBuffer {
    pub channels: usize,
    pub capacity: usize,
    pub len: usize,
    /// When the buffer's first sample was, by the stream's clock (`Continuous::timing`): the sample's number, counting
    /// at the stream's sample rate from the clock's zero (so a downsampled stream counts its own samples). The next
    /// buffer starts at `start + len`, unless samples were lost in between: a gap.
    pub start: u64,
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
            (self.len, self.start) = (input.len.min(self.capacity), input.start);
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

impl DataStreamType for Continuous {
    type Buffer = ContinuousBuffer;

    /// Room for a quarter more than a pass's worth, so a source can catch up after a hiccup (samples arriving
    /// late, say).
    fn buffer(&self, duration: f64) -> ContinuousBuffer {
        let capacity = per_pass(self.sample_rate, duration) * 5 / 4;
        let channels = self.channels as usize;
        ContinuousBuffer { channels, capacity, len: 0, start: 0, data: vec![0.0; channels * capacity] }
    }

    fn bytes(buffer: &ContinuousBuffer) -> usize {
        buffer.data.capacity() * size_of::<f32>()
    }

    fn copy(from: &ContinuousBuffer, to: &mut ContinuousBuffer) {
        (to.len, to.start) = (from.len.min(to.capacity), from.start);
        let len = to.len;
        for channel in 0..from.channels.min(to.channels) {
            to.channel_mut(channel).copy_from_slice(&from.channel(channel)[..len]);
        }
    }

    fn empty(buffer: &mut ContinuousBuffer) {
        buffer.len = 0;
    }
}

/// Regularly sampled complex values, such as the analytic signal that a Hilbert Transform makes from continuous data.
/// Each sample's argument is the signal's phase and its magnitude is the signal's envelope.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ComplexContinuous {
    pub channels: u32,
    pub sample_rate: f32,
    /// The clock that times the samples, as for `Continuous::clock`.
    pub clock: ClockId,
}

impl EdgeType for ComplexContinuous {
    const INFO: EdgeInfo = EdgeInfo {
        name: "complex continuous",
        colour: Color32::from_rgb(70, 205, 195),
        description: "Regularly sampled complex values, on any number of channels: the analytic signal of continuous \
                      data, say, whose phase and envelope a Phase Viewer shows.",
    };

    fn summary(&self) -> String {
        format!("{} ch · {}", self.channels, format_rate(self.sample_rate))
    }
}

/// A pass's worth of complex continuous data, laid out as `ContinuousBuffer` is, with the real and imaginary parts in
/// separate arrays: for each channel, a row of up to `capacity` samples, of which the first `len` are this pass's.
pub struct ComplexContinuousBuffer {
    pub channels: usize,
    pub capacity: usize,
    pub len: usize,
    /// The buffer's first sample's number, as `ContinuousBuffer::start`.
    pub start: u64,
    /// The real parts, channel after channel, each `capacity` long.
    pub re: Vec<f32>,
    /// The imaginary parts, laid out as `re`.
    pub im: Vec<f32>,
}

impl ComplexContinuousBuffer {
    /// Channel `channel`'s real and imaginary parts for this pass.
    pub fn channel(&self, channel: usize) -> (&[f32], &[f32]) {
        let start = channel * self.capacity;
        (&self.re[start..start + self.len], &self.im[start..start + self.len])
    }

    /// Channel `channel`'s real and imaginary parts for this pass, to fill.
    pub fn channel_mut(&mut self, channel: usize) -> (&mut [f32], &mut [f32]) {
        let start = channel * self.capacity;
        (&mut self.re[start..start + self.len], &mut self.im[start..start + self.len])
    }
}

impl DataStreamType for ComplexContinuous {
    type Buffer = ComplexContinuousBuffer;

    /// Room for a quarter more than a pass's worth, as `Continuous` has.
    fn buffer(&self, duration: f64) -> ComplexContinuousBuffer {
        let capacity = per_pass(self.sample_rate, duration) * 5 / 4;
        let channels = self.channels as usize;
        let size = channels * capacity;
        ComplexContinuousBuffer { channels, capacity, len: 0, start: 0, re: vec![0.0; size], im: vec![0.0; size] }
    }

    fn bytes(buffer: &ComplexContinuousBuffer) -> usize {
        (buffer.re.capacity() + buffer.im.capacity()) * size_of::<f32>()
    }

    fn copy(from: &ComplexContinuousBuffer, to: &mut ComplexContinuousBuffer) {
        (to.len, to.start) = (from.len.min(to.capacity), from.start);
        let len = to.len;
        for channel in 0..from.channels.min(to.channels) {
            let (re, im) = from.channel(channel);
            let (to_re, to_im) = to.channel_mut(channel);
            to_re.copy_from_slice(&re[..len]);
            to_im.copy_from_slice(&im[..len]);
        }
    }

    fn empty(buffer: &mut ComplexContinuousBuffer) {
        buffer.len = 0;
    }
}

/// A digital event: a line going high or low.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    /// When the line changed, by the stream's clock (`Events::clock`): a tick's number (whole, and exact as an `f64`
    /// for any time a run could take), or seconds.
    pub at: f64,
    pub line: u8,
    pub high: bool,
}

/// Events, in order, and the stretch of the stream's clock they cover, from `start` up to `end`: no line changed in
/// it but as `events` say. The next buffer's stretch starts at `end`, unless some was lost between them: a gap. A
/// source timed in seconds only knows as far as its last news, so its stretch can be empty.
pub struct EventsBuffer {
    pub events: Vec<Event>,
    pub start: f64,
    pub end: f64,
}

impl DataStreamType for Events {
    type Buffer = EventsBuffer;

    fn buffer(&self, _duration: f64) -> EventsBuffer {
        EventsBuffer { events: Vec::with_capacity(64 * self.lines.max(1) as usize), start: 0.0, end: 0.0 }
    }

    fn bytes(buffer: &EventsBuffer) -> usize {
        buffer.events.capacity() * size_of::<Event>()
    }

    fn copy(from: &EventsBuffer, to: &mut EventsBuffer) {
        to.events.clone_from(&from.events);
        (to.start, to.end) = (from.start, from.end);
    }

    fn empty(buffer: &mut EventsBuffer) {
        buffer.events.clear();
        buffer.start = buffer.end;
    }
}

/// Which channels a channel selector passes on: a set such as "1-4, 9" (see `reference::parse_channels`), and
/// whether it's applied (if not, all are passed on, but the set's kept, to choose from).
#[derive(Clone, PartialEq, Debug)]
pub struct ChannelSelection {
    pub apply: bool,
    pub channels: String,
}

/// The command that gives a node publishing a channel selection, such as a Channel Selector, a new selection.
#[derive(Debug)]
pub struct SetChannelSelection(pub ChannelSelection);

impl EdgeType for ChannelSelection {
    const INFO: EdgeInfo = EdgeInfo {
        name: "channel selection",
        colour: Color32::from_rgb(215, 175, 120),
        description: "Which channels to pass on, and whether that's applied.",
    };

    fn summary(&self) -> String {
        if self.apply { reference::channels_summary(&self.channels) } else { "all channels".into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephys_rs::engine::EdgeType;

    #[test]
    fn summary() {
        assert_eq!(Continuous { channels: 32, sample_rate: 30000.0, clock: ClockId::TEST }.summary(), "32 ch · 30 kHz");
    }
}
