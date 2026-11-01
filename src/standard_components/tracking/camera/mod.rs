//! Cameras: the Camera node (`node`), capturing on a thread of its own (`capture`), and the backends it
//! captures from. Each backend produces 8-bit grey frames tagged with the camera's own frame id and timestamp.

pub mod capture;
mod fake;
pub mod node;
mod spinnaker;

use std::io;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

use crate::core::engine::{DataStreamType, EdgeInfo, EdgeType};
pub use fake::FakeCamera;
#[cfg(test)]
pub use fake::serve_test_scene;
pub use spinnaker::SpinnakerCamera;

/// A camera's stats, as a data stream: how capture is going, and what the camera is and supports.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CameraStats;

impl EdgeType for CameraStats {
    const INFO: EdgeInfo = EdgeInfo {
        name: "camera stats",
        colour: Color32::from_rgb(170, 175, 195),
        description: "How a camera is doing (frames, rate, drops), and what it is and supports.",
    };

    fn summary(&self) -> String {
        String::new()
    }
}

/// How a camera's capture is going, as of a pass.
#[derive(Clone, Debug, Default)]
pub struct CaptureStats {
    pub status: capture::Status,
    /// Frames skipped for being the wrong size, and the size of the last of them.
    pub wrong_size: u64,
    pub last_wrong_size: [u32; 2],
    /// Frames skipped because more arrived in a pass than it had room for.
    pub missed: u64,
}

impl CaptureStats {
    /// Copies `other`, reusing this one's memory (for its text).
    pub fn copy_from(&mut self, other: &CaptureStats) {
        self.status.copy_from(&other.status);
        (self.wrong_size, self.last_wrong_size, self.missed) = (other.wrong_size, other.last_wrong_size, other.missed);
    }
}

impl DataStreamType for CameraStats {
    type Buffer = CaptureStats;

    fn buffer(&self, _duration: f64) -> CaptureStats {
        CaptureStats::default()
    }

    fn bytes(_buffer: &CaptureStats) -> usize {
        size_of::<CaptureStats>()
    }
}

/// Camera settings that another node (the Tracker View) can adjust.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraSettings {
    /// Frames per second: also the rate of the video stream, so changing it restarts the run.
    pub frame_rate: f64,
    /// Microseconds.
    pub exposure: Control,
    /// Decibels.
    pub gain: Control,
}

impl Default for CameraSettings {
    fn default() -> Self {
        CameraSettings { frame_rate: 50.0, exposure: Control::Unchanged, gain: Control::Unchanged }
    }
}

/// How to set a camera feature that can be automatic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "value", rename_all = "snake_case")]
pub enum Control {
    /// Leave it as the camera is configured (e.g. in SpinView).
    #[default]
    Unchanged,
    Auto,
    Manual(f64),
}

impl EdgeType for CameraSettings {
    const INFO: EdgeInfo = EdgeInfo {
        name: "camera",
        colour: Color32::from_rgb(150, 160, 255),
        description: "A camera's frame rate, exposure and gain.",
    };

    fn summary(&self) -> String {
        format!("{} fps", self.frame_rate)
    }
}

pub struct Frame {
    /// Frame counter as reported by the camera (gaps imply dropped frames).
    pub id: u64,
    /// Seconds, in the camera's clock.
    pub timestamp: f64,
    pub width: u32,
    pub height: u32,
    /// `width` × `height` bytes of grey, row by row.
    pub pixels: Vec<u8>,
}

pub trait Camera: Send {
    /// Waits briefly for the next frame. `Ok(None)` means nothing arrived yet; the caller should check whether
    /// it has been asked to stop and then call again.
    fn next_frame(&mut self) -> io::Result<Option<Frame>>;

    /// E.g. the camera model and serial number.
    fn description(&self) -> String {
        String::new()
    }

    /// Problems configuring the camera that don't stop it working.
    fn warnings(&self) -> Vec<String> {
        Vec::new()
    }

    /// Applies exposure and gain (the frame rate is only set when the camera is opened).
    fn apply(&mut self, _settings: &CameraSettings) -> io::Result<()> {
        Ok(())
    }

    /// Which settings this camera supports, their ranges and current values.
    fn controls(&self) -> CameraControls {
        CameraControls::default()
    }
}

/// Supported settings, as reported by the camera. `None` means not supported.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraControls {
    /// Whether the camera took the requested frame rate. If not, it runs at whatever it's configured for.
    pub frame_rate_set: bool,
    pub frame_rate: Option<Range>,
    pub exposure: Option<Range>,
    pub gain: Option<Range>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    /// What the camera reports now, which may differ from what was asked (e.g. under auto exposure).
    pub current: f64,
}

/// Which camera to capture from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// Raw frames streamed over TCP by `synthetic_streams/fake_camera.py`: a mouse with two head LEDs in an
    /// arena.
    Fake { address: String },
    /// A Teledyne FLIR camera, via the Spinnaker SDK.
    Spinnaker {
        /// Blank for the first camera found.
        serial: String,
        /// The Spinnaker C library, if it isn't found automatically; blank to search.
        library: String,
    },
}

impl Default for Source {
    fn default() -> Self {
        Source::Fake { address: FAKE_ADDRESS.into() }
    }
}

/// Where `fake_camera.py` serves frames by default.
pub const FAKE_ADDRESS: &str = "127.0.0.1:27030";

/// Opens the camera.
pub fn open(source: &Source, settings: &CameraSettings) -> io::Result<Box<dyn Camera>> {
    let blank_as_none = |s: &str| Some(s.trim()).filter(|s| !s.is_empty()).map(str::to_string);
    Ok(match source {
        // The fake camera's rate is set by fake_camera.py --fps.
        Source::Fake { address } => Box::new(FakeCamera::connect(address)?),
        Source::Spinnaker { serial, library } => Box::new(SpinnakerCamera::open(
            blank_as_none(serial).as_deref(),
            settings.frame_rate,
            blank_as_none(library).as_deref(),
        )?),
    })
}
