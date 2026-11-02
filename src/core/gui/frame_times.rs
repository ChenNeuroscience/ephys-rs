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

pub struct FrameTimes {
    started: Instant,
    pub frames: VecDeque<Frame>,
    /// The frame being built.
    current: Option<Frame>,
    /// When the graph started running, if it is (seconds since the app started), for timing passes alike.
    pub run_started: Option<f64>,
    /// Not timing frames, and the time it stopped at (so tests' snapshots don't change with how fast they run).
    paused: Option<f64>,
}

impl Default for FrameTimes {
    fn default() -> Self {
        FrameTimes { started: Instant::now(), frames: VecDeque::new(), current: None, run_started: None, paused: None }
    }
}

impl FrameTimes {
    /// Seconds since the app started.
    pub fn now(&self) -> f64 {
        self.paused.unwrap_or_else(|| self.started.elapsed().as_secs_f64())
    }

    /// Stops timing frames, forgetting those so far, and stops the clock.
    #[cfg(test)]
    pub fn pause(&mut self) {
        self.frames.clear();
        self.current = None;
        self.paused = Some(0.0);
    }

    /// Starts timing a frame, giving the last its layout and painting time, from its total CPU time (in seconds).
    pub fn begin(&mut self, last_cpu: Option<f32>) {
        if self.paused.is_some() {
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

    /// Adds `ms` to `part` of this frame.
    pub fn add(&mut self, part: &str, ms: f32) {
        let Some(frame) = &mut self.current else { return };
        match frame.parts.iter_mut().find(|p| p.0 == part) {
            Some(p) => p.1 += ms,
            None => frame.parts.push((part.to_owned(), ms)),
        }
    }
}
