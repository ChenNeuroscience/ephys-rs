//! Electrophysiology: continuous voltages and digital events, from an acquisition board, re-referenced,
//! filtered, downsampled, scaled and viewed.

pub mod band_pass;
pub mod board;
pub mod channel_selector;
pub mod continuous_gain;
pub mod downsample;
pub mod lfp_viewer;
pub mod reference;
pub mod spectrum;
pub mod ttl_viewer;

use eframe::egui::Color32;

use super::per_pass;
use super::widgets::format_rate;
use ephys_rs::engine::{DataStreamType, EdgeInfo, EdgeType};

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
    /// The rate of the samples the events are timed by.
    pub sample_rate: f32,
}

impl EdgeType for Events {
    const INFO: EdgeInfo = EdgeInfo {
        name: "events",
        colour: Color32::from_rgb(190, 130, 255),
        description: "Digital events, e.g. TTL inputs.",
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

impl DataStreamType for Continuous {
    type Buffer = ContinuousBuffer;

    /// Room for a quarter more than a pass's worth, so a source can catch up after a hiccup (samples arriving
    /// late, say).
    fn buffer(&self, duration: f64) -> ContinuousBuffer {
        let capacity = per_pass(self.sample_rate, duration) * 5 / 4;
        let channels = self.channels as usize;
        ContinuousBuffer { channels, capacity, len: 0, data: vec![0.0; channels * capacity] }
    }

    fn bytes(buffer: &ContinuousBuffer) -> usize {
        buffer.data.capacity() * size_of::<f32>()
    }

    fn copy(from: &ContinuousBuffer, to: &mut ContinuousBuffer) {
        to.len = from.len.min(to.capacity);
        let len = to.len;
        for channel in 0..from.channels.min(to.channels) {
            to.channel_mut(channel).copy_from_slice(&from.channel(channel)[..len]);
        }
    }
}

/// A digital event: a line going high or low, at a sample within the pass.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub sample: u32,
    pub line: u8,
    pub high: bool,
    /// When it happened, in seconds by its source's own clock, if it has one: otherwise it's timed by its sample.
    pub time: Option<f64>,
}

pub struct EventsBuffer {
    pub events: Vec<Event>,
    /// How many samples the pass covers (for timing events across passes).
    pub pass_samples: usize,
}

impl DataStreamType for Events {
    type Buffer = EventsBuffer;

    fn buffer(&self, _duration: f64) -> EventsBuffer {
        EventsBuffer { events: Vec::with_capacity(64 * self.lines.max(1) as usize), pass_samples: 0 }
    }

    fn bytes(buffer: &EventsBuffer) -> usize {
        buffer.events.capacity() * size_of::<Event>()
    }

    fn copy(from: &EventsBuffer, to: &mut EventsBuffer) {
        to.events.clone_from(&from.events);
        to.pass_samples = from.pass_samples;
    }
}

/// Which channels a channel selector passes on: a set such as "1-4, 9" (see `reference::parse_channels`), and
/// whether it's applied (if not, all are passed on, but the set's kept, to choose from).
#[derive(Clone, PartialEq, Debug)]
pub struct ChannelSelection {
    pub apply: bool,
    pub channels: String,
}

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
        assert_eq!(Continuous { channels: 32, sample_rate: 30000.0 }.summary(), "32 ch · 30 kHz");
    }
}
