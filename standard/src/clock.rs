//! Clocks: what data streams' times are counted by.
//!
//! A clock is a convention as much as code. Each data stream says which clock its times are by (its `ClockId`), and two
//! times by the same clock come from the same physical clock, counted from the same zero (whenever that was: a board's
//! first sample, say, or a camera's power-on). So times by the same clock can be compared, and times by different
//! clocks can't (lining clocks up is a separate matter: see Sync Positions).
//!
//! A clock's id is fixed for a snapshot, so it's part of a stream (the graph works it out, see `Node::data_stream_out`),
//! while the times themselves come in the stream's buffers, pass by pass. Each source names the clocks it times its
//! data by, from its node's id and a number of its own (see `ClockId`); a node passing data on passes its clock on.
//! The clocks so far are an acquisition board's (its samples), a camera's (its frames' timestamps) and a remote
//! computer's (the VR computer's).
//!
//! A stream counts its times in units of its clock's timebase: so many a second (see `Clock`). The same clock can be
//! counted in different units in different streams (a board's samples at 30 kHz, and its EEG downsampled to 5 kHz),
//! and as the zero's the same, converting between them is only a matter of scale. How a stream gives its times
//! depends on the stream:
//!
//! - Continuous data (`Continuous`, `ComplexContinuous`) is counted in samples (its sample rate is its timebase), and
//!   each buffer says when its first sample was (`ContinuousBuffer::start`); the rest follow one a sample.
//! - Spikes, slices of continuous data, likewise: each spike says when its waveform's first sample was, in the
//!   continuous data's samples.
//! - Events, positions, video frames and log lines each have a time of their own.
//! - A time stream (`Time`) has only times: the stretch of its clock each pass covers, so a writer can say when
//!   recording started by that clock, whatever else it's given.
//!
//! A buffer that doesn't start where the stream's last ended shows a gap: data lost by the hardware, its driver or a
//! full buffer.

use eframe::egui::Color32;

use super::widgets::format_rate;
use ephys_rs::engine::{DataStreamType, EdgeInfo, EdgeType, NodeId};

/// Which clock a stream's times are by (see the module's docs). A source names each clock it has from its node's `id`
/// and an `index` of its own: 0 for its only clock, say, or a count of the times it's started afresh, its zero moved.
/// Clocks only need telling apart within a snapshot.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ClockId {
    pub node: NodeId,
    pub index: u32,
}

impl ClockId {
    pub const fn new(node: NodeId, index: u32) -> Self {
        ClockId { node, index }
    }

    /// A clock for tests to time streams by.
    #[cfg(test)]
    pub const TEST: ClockId = ClockId::new(NodeId(1), 0);
}

/// A clock as a stream counts times by it: which clock (`id`), and its timebase, how many of the stream's units make a
/// second (`per_second`): a board's sample rate, say, or 1 for times in seconds.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Clock {
    pub id: ClockId,
    pub per_second: f64,
}

impl Clock {
    pub const fn new(id: ClockId, per_second: f64) -> Self {
        Clock { id, per_second }
    }

    /// Clock `id`, counted in seconds.
    pub const fn seconds_of(id: ClockId) -> Self {
        Clock { id, per_second: 1.0 }
    }

    /// A time by this clock (`at`, in its units), in seconds.
    pub fn seconds(self, at: f64) -> f64 {
        at / self.per_second.max(f64::MIN_POSITIVE)
    }

    /// Whether this counts whole ticks (a board's samples, say) rather than seconds.
    pub fn ticks(self) -> bool {
        self.per_second != 1.0
    }
}

/// Times only: the stretch of a clock each pass covers, from a source (an acquisition board, say). A writer reads this
/// to say when recording started by the source's clock, whatever else it's given; see `TimeBuffer`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Time {
    pub clock: Clock,
}

impl EdgeType for Time {
    const INFO: EdgeInfo = EdgeInfo {
        name: "time",
        colour: Color32::from_rgb(200, 200, 200),
        description: "Only times: the stretch of a source's clock each pass covers, to say when recording started.",
    };

    fn summary(&self) -> String {
        if self.clock.ticks() { format_rate(self.clock.per_second as f32) } else { "seconds".into() }
    }
}

/// A pass's stretch of a clock, `start` to `end` (by the stream's `Clock`): a board's first sample this pass, say, and
/// the one after its last. A pass with no data covers nothing (`start` is `end`), and a stretch not starting where the
/// last ended shows a gap.
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
