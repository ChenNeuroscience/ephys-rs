//! Clocks: what data streams' times are counted by.
//!
//! Data comes from several pieces of hardware (an acquisition board, a camera, a remote computer), each keeping time by
//! a clock of its own. A data stream names the clock its times are by with a `ClockId`. By convention, two times with
//! the same clock id come from the same physical clock and count from the same zero, whenever that zero was (a board's
//! first sample, say, or a camera's power-on). So times with the same clock id can be compared, and times with
//! different clock ids can't. Lining different clocks up is a separate matter (see Post-Process Positions LTX).
//!
//! A clock id is fixed for a snapshot, so the id belongs to the stream, which the graph works out (see
//! `Node::data_stream_out`); the times themselves come in the stream's buffers, pass by pass. A source node makes the
//! ids for the clocks its data is timed by, from the node's id and a number of the node's own (see `ClockId`). A node
//! that passes data on passes the clock id on with the data. The clocks so far are an acquisition board's (counting
//! samples), a camera's (its frames' timestamps) and the VR computer's.
//!
//! Each stream counts its times in units of a timebase: so many units a second (see `Clock`). Streams can count the
//! same clock in different units (a board's samples at 30 kHz, and an EEG downsampled from them to 5 kHz). The zero is
//! the same either way, so converting between the two is only a matter of scale. Each kind of stream gives its times
//! in its own way:
//!
//! - Continuous data (`Continuous`, `ComplexContinuous`) counts in samples (the sample rate is the timebase). Each
//!   buffer gives the time of the buffer's first sample (`ContinuousBuffer::start`), and the samples after it follow
//!   one a sample.
//! - Spikes are slices of continuous data, timed in that data's samples: each spike gives the sample the spike peaked
//!   at, and the waveform starts `Spikes::pre` samples before that.
//! - Events, positions, video frames and log lines each have a time of their own.
//! - A time stream (`Time`) has nothing but times: each pass gives the stretch of the clock the pass covers, so that a
//!   writer can tell when recording started by that clock, whatever other data the writer has.
//!
//! When a buffer doesn't start where the stream's previous buffer ended, the difference is a gap: data lost by the
//! hardware, the hardware's driver, or a buffer that was full.

use eframe::egui::Color32;

use super::widgets::format_rate;
use ephys_rs::engine::{DataStreamType, EdgeInfo, EdgeType, NodeId};

/// A data stream names the clock its times are by (see the module's docs). This is the clock's name: the `id` of the
/// source node that made it, and an `index` the source node chooses, such as 0 for the node's only clock, or a count of
/// the times the node has restarted the clock from a new zero. Clock ids only need telling apart within a snapshot.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ClockId {
    pub node: NodeId,
    pub index: u32,
}

impl ClockId {
    pub const fn new(node: NodeId, index: u32) -> Self {
        ClockId { node, index }
    }

    /// Tests time their streams by this clock id.
    #[cfg(test)]
    pub const TEST: ClockId = ClockId::new(NodeId(1), 0);
}

/// A stream counts the times of a clock in units of its own. This says which clock (`id`) and the stream's timebase
/// (`per_second`): how many of those units make a second, such as a board's sample rate, or 1 for times in seconds.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Clock {
    pub id: ClockId,
    pub per_second: f64,
}

impl Clock {
    pub const fn new(id: ClockId, per_second: f64) -> Self {
        Clock { id, per_second }
    }

    /// The clock with this `id`, counted in seconds.
    pub const fn seconds_of(id: ClockId) -> Self {
        Clock { id, per_second: 1.0 }
    }

    /// Converts a time by this clock (`at`, in the timebase's units) to seconds.
    pub fn seconds(self, at: f64) -> f64 {
        at / self.per_second.max(f64::MIN_POSITIVE)
    }

    /// Whether the timebase counts whole ticks (a board's samples, say) rather than seconds.
    pub fn ticks(self) -> bool {
        self.per_second != 1.0
    }
}

/// A source (an acquisition board, say) can offer a stream of nothing but times: for each pass, the stretch of the
/// source's clock that the pass covers (see `TimeBuffer`). A writer reads the times to tell when recording started by
/// that clock, whatever other data the writer has.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Time {
    pub clock: Clock,
}

impl EdgeType for Time {
    const INFO: EdgeInfo = EdgeInfo {
        name: "time",
        colour: Color32::from_rgb(200, 200, 200),
        description: "Timing only: how far a device's clock has run, so writers know when recording started.",
    };

    fn summary(&self) -> String {
        if self.clock.ticks() { format_rate(self.clock.per_second as f32) } else { "seconds".into() }
    }
}

/// A time stream's buffer holds the stretch of the clock a pass covers, from `start` to `end`, in the stream's
/// timebase. For a board, `start` is the pass's first sample and `end` the sample after the pass's last. A pass with no
/// data covers nothing (`start` is `end`), and a stretch that doesn't start where the previous one ended is a gap.
#[derive(Clone, Copy, Default, Debug)]
pub struct TimeBuffer {
    pub start: f64,
    pub end: f64,
}

impl DataStreamType for Time {
    type Buffer = TimeBuffer;

    fn buffer(&self, _duration: f64) -> TimeBuffer {
        TimeBuffer::default()
    }

    fn bytes(_buffer: &TimeBuffer) -> usize {
        0
    }

    fn copy(from: &TimeBuffer, to: &mut TimeBuffer) {
        *to = *from;
    }

    fn empty(buffer: &mut TimeBuffer) {
        buffer.start = buffer.end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_by_a_clock_are_seconds_at_its_timebase() {
        let samples = Clock::new(ClockId::TEST, 30000.0);
        assert_eq!(samples.seconds(60000.0), 2.0);
        assert!(samples.ticks());
        let seconds = Clock::seconds_of(ClockId::TEST);
        assert_eq!(seconds.seconds(2.5), 2.5);
        assert!(!seconds.ticks());
    }
}
