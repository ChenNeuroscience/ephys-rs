use std::io;
use std::time::{Duration, Instant};

use super::{Camera, CameraControls, Frame, Range};
use crate::tracking::{CameraSettings, Control};

// Simulated exposure (µs) and gain (dB): brightness scales with exposure and with gain as amplitude.
const DEFAULT_EXPOSURE: f64 = 10_000.0;
const EXPOSURE_RANGE: (f64, f64) = (100.0, 50_000.0);
const GAIN_RANGE: (f64, f64) = (0.0, 24.0);

/// Two LEDs (a big bright one and a smaller one behind it) circling the frame, at the frame rate asked for.
pub struct SyntheticCamera {
    fps: f64,
    exposure: f64,
    gain: f64,
    width: u32,
    height: u32,
    interval: Duration,
    start: Instant,
    next_due: Instant,
    id: u64,
}

impl SyntheticCamera {
    pub fn new(width: u32, height: u32, fps: f64) -> Self {
        let now = Instant::now();
        let fps = fps.max(1.0);
        SyntheticCamera {
            fps,
            exposure: DEFAULT_EXPOSURE,
            gain: 0.0,
            width,
            height,
            interval: Duration::from_secs_f64(1.0 / fps),
            start: now,
            next_due: now,
            id: 0,
        }
    }
}

impl Camera for SyntheticCamera {
    fn next_frame(&mut self) -> io::Result<Option<Frame>> {
        let now = Instant::now();
        if now < self.next_due {
            std::thread::sleep((self.next_due - now).min(Duration::from_millis(50)));
            if Instant::now() < self.next_due {
                return Ok(None);
            }
        }
        self.next_due += self.interval;

        let t = self.id as f64 * self.interval.as_secs_f64();
        let (w, h) = (self.width as f64, self.height as f64);
        let theta = t * 0.7;
        let (x, y) = (w / 2.0 + w / 3.0 * theta.cos(), h / 2.0 + h / 3.0 * (theta * 1.3).sin());
        let heading = theta + std::f64::consts::FRAC_PI_2;

        let brightness = self.exposure / DEFAULT_EXPOSURE * 10f64.powf(self.gain / 20.0);
        let level = |v: f64| (v * brightness).min(255.0) as u8;
        let mut pixels = vec![level(20.0); (self.width * self.height) as usize];
        disc(&mut pixels, self.width, self.height, [x, y], 5.0, level(255.0));
        disc(
            &mut pixels,
            self.width,
            self.height,
            [x - 25.0 * heading.cos(), y - 25.0 * heading.sin()],
            3.0,
            level(240.0),
        );

        let frame = Frame {
            id: self.id,
            timestamp: self.start.elapsed().as_secs_f64(),
            width: self.width,
            height: self.height,
            pixels,
        };
        self.id += 1;
        Ok(Some(frame))
    }

    fn description(&self) -> String {
        "Synthetic test scene".into()
    }

    fn apply(&mut self, settings: &CameraSettings) -> io::Result<()> {
        self.exposure = match settings.exposure {
            Control::Unchanged => self.exposure,
            Control::Auto => DEFAULT_EXPOSURE,
            Control::Manual(v) => v.clamp(EXPOSURE_RANGE.0, EXPOSURE_RANGE.1),
        };
        self.gain = match settings.gain {
            Control::Unchanged => self.gain,
            Control::Auto => 0.0,
            Control::Manual(v) => v.clamp(GAIN_RANGE.0, GAIN_RANGE.1),
        };
        Ok(())
    }

    fn controls(&self) -> CameraControls {
        CameraControls {
            frame_rate_set: true,
            frame_rate: Some(Range { min: 1.0, max: 500.0, current: self.fps }),
            exposure: Some(Range { min: EXPOSURE_RANGE.0, max: EXPOSURE_RANGE.1, current: self.exposure }),
            gain: Some(Range { min: GAIN_RANGE.0, max: GAIN_RANGE.1, current: self.gain }),
        }
    }
}

/// Fills a disc of `radius` pixels about `centre` with `value`.
fn disc(pixels: &mut [u8], width: u32, height: u32, centre: [f64; 2], radius: f64, value: u8) {
    let rows = (centre[1] - radius).floor().max(0.0) as u32..((centre[1] + radius).ceil() as u32 + 1).min(height);
    let columns = (centre[0] - radius).floor().max(0.0) as u32..((centre[0] + radius).ceil() as u32 + 1).min(width);
    for y in rows {
        for x in columns.clone() {
            if (x as f64 - centre[0]).powi(2) + (y as f64 - centre[1]).powi(2) <= radius * radius {
                pixels[(y * width + x) as usize] = value;
            }
        }
    }
}
