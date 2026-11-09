//! How long the GUI takes over each frame, part by part (the toolbar, each pane, the floating windows...), for the
//! dev tools' performance chart. The parts are timed as they're built; what eframe then spends laying out and
//! painting the frame is the rest of its CPU time, which it reports a frame later.

use std::collections::VecDeque;
use std::time::Instant;

/// Seconds of frames kept.
pub const KEPT: f64 = 12.0;
/// The part that's the rest of a frame's CPU time.
pub const PAINTING: &str = "Layout and painting";

/// A frame: when it began (seconds since the app started), and its parts' times (ms).
pub struct Frame {
    pub at: f64,
    pub parts: Vec<(String, f32)>,
}

/// Something that changed the run, marked on the chart: when it was asked for, then when the first pass used it.
pub struct Mark {
    pub at: f64,
    pub kind: MarkKind,
    /// Short, to label the mark; and what changed, in full.
    pub label: String,
    pub detail: String,
    /// Whether this is the first pass using the change, rather than the change.
    pub first_use: bool,
}

/// A change no pass has used yet.
struct Waiting {
    version: u64,
    at: f64,
    kind: MarkKind,
    label: String,
    detail: String,
}

/// What a mark on the timeline is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkKind {
    /// Recording started or stopped.
    Recording,
    /// A new version that runs differently: a new epoch, with fresh state.
    Epoch,
    /// A new version the run carried on with (new settings, say).
    Version,
}

/// The GUI's frames and the run's passes over the last while, and marks for changes, for the timeline in the dev
/// tools.
pub struct FrameTimes {
    started: Instant,
    pub frames: VecDeque<Frame>,
    pub marks: VecDeque<Mark>,
    /// The changes marked that no pass has used yet, oldest first.
    waiting: VecDeque<Waiting>,
    /// The frame being built.
    current: Option<Frame>,
    /// When the graph started running, if it is (seconds since the app started), for timing passes alike.
    pub run_started: Option<f64>,
    /// Not timing frames, and the time it stopped at (so tests' snapshots don't change with how fast they run).
    frozen: Option<f64>,
}

impl Default for FrameTimes {
    fn default() -> Self {
        FrameTimes {
            started: Instant::now(),
            frames: VecDeque::new(),
            marks: VecDeque::new(),
            waiting: VecDeque::new(),
            current: None,
            run_started: None,
            frozen: None,
        }
    }
}

impl FrameTimes {
    /// Seconds since the app started.
    pub fn now(&self) -> f64 {
        self.frozen.unwrap_or_else(|| self.started.elapsed().as_secs_f64())
    }

    /// Stops timing frames, forgetting those so far, and stops the clock: for tests, which can't have times change.
    #[cfg(test)]
    pub fn freeze(&mut self) {
        self.frames.clear();
        self.current = None;
        self.frozen = Some(0.0);
    }

    /// Starts timing a frame, giving the last its layout and painting time, from its total CPU time (in seconds).
    pub fn begin(&mut self, last_cpu: Option<f32>) {
        if self.frozen.is_some() {
            return;
        }
        if let Some(frame) = self.current.take() {
            if let Some(cpu) = last_cpu {
                let parts: f32 = frame.parts.iter().map(|p| p.1).sum();
                let mut frame = frame;
                frame.parts.push((PAINTING.into(), (cpu * 1000.0 - parts).max(0.0)));
                self.frames.push_back(frame);
            } else {
                self.frames.push_back(frame);
            }
        }
        let at = self.now();
        while self.frames.front().is_some_and(|f| f.at < at - KEPT) {
            self.frames.pop_front();
        }
        self.current = Some(Frame { at, parts: Vec::new() });
    }

    /// Marks a change to the run, now: `version`, which passes will use.
    pub fn mark(&mut self, version: u64, kind: MarkKind, label: String, detail: String) {
        let at = self.now();
        self.waiting.push_back(Waiting { version, at, kind, label: label.clone(), detail: detail.clone() });
        self.add_mark(Mark { at, kind, label, detail, first_use: false });
    }

    /// Marks pass `sequence`, started at `at`, as the first to use `version`, and so the changes up to it.
    pub fn first_use(&mut self, version: u64, sequence: u64, at: Instant) {
        let at = self.frozen.unwrap_or_else(|| at.saturating_duration_since(self.started).as_secs_f64());
        while let Some(change) = self.waiting.pop_front_if(|w| w.version <= version) {
            let later = ((at - change.at) * 1000.0).max(0.0);
            let detail =
                format!("{}: first used by pass {sequence}, {later:.0} ms later\n{}", change.label, change.detail);
            self.add_mark(Mark { at, kind: change.kind, label: change.label, detail, first_use: true });
        }
    }

    /// Passes starting from here are a new run's: nothing's waiting for them.
    pub fn run_restarted(&mut self) {
        self.waiting.clear();
    }

    fn add_mark(&mut self, mark: Mark) {
        while self.marks.front().is_some_and(|m| m.at < mark.at - KEPT) {
            self.marks.pop_front();
        }
        self.marks.push_back(mark);
    }

    /// Adds `ms` to `part` of this frame.
    pub fn add(&mut self, part: &str, ms: f32) {
        let Some(frame) = &mut self.current else { return };
        match frame.parts.iter_mut().find(|p| p.0 == part) {
            Some(p) => p.1 += ms,
            None => frame.parts.push((part.to_owned(), ms)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_marked_again_when_a_pass_first_uses_them() {
        let mut times = FrameTimes::default();
        let change = |times: &mut FrameTimes, version: u64| {
            times.mark(version, MarkKind::Version, format!("v{version}"), "Settings".into());
        };
        change(&mut times, 2);
        change(&mut times, 3);
        change(&mut times, 4);
        // Passes went from before them to version 3: 2 never ran, but it's in use from there too.
        times.first_use(3, 7, Instant::now());
        let first_uses = |times: &FrameTimes| times.marks.iter().filter(|m| m.first_use).count();
        assert_eq!((times.marks.len(), first_uses(&times)), (5, 2));
        assert!(times.marks.back().unwrap().detail.starts_with("v3: first used by pass 7, "));
        // A version with nothing marked (something moved, say) marks nothing.
        times.first_use(3, 8, Instant::now());
        assert_eq!(first_uses(&times), 2);
        // A run stopped never gets to it.
        times.run_restarted();
        times.first_use(4, 0, Instant::now());
        assert_eq!(first_uses(&times), 2);
    }
}
