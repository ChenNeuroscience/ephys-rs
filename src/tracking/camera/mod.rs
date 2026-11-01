//! Camera backends. Each produces 8-bit grey frames tagged with the camera's own frame id and timestamp.

mod fake;
mod spinnaker;
mod synthetic;

use std::io;

use serde::{Deserialize, Serialize};

use super::CameraSettings;
pub use fake::FakeCamera;
pub use spinnaker::SpinnakerCamera;
pub use synthetic::SyntheticCamera;

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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// A test scene with two LEDs going round, made here.
    #[default]
    Synthetic,
    /// Raw frames streamed over TCP by `tools/fake_camera.py`: a mouse with two head LEDs in an arena.
    Fake { address: String },
    /// A Teledyne FLIR camera, via the Spinnaker SDK.
    Spinnaker {
        /// Blank for the first camera found.
        serial: String,
        /// The Spinnaker C library, if it isn't found automatically; blank to search.
        library: String,
    },
}

/// Opens the camera. `size` is the frame size the synthetic camera makes.
pub fn open(source: &Source, settings: &CameraSettings, size: [u32; 2]) -> io::Result<Box<dyn Camera>> {
    let blank_as_none = |s: &str| Some(s.trim()).filter(|s| !s.is_empty()).map(str::to_string);
    Ok(match source {
        Source::Synthetic => Box::new(SyntheticCamera::new(size[0], size[1], settings.frame_rate)),
        // The fake camera's rate is set by fake_camera.py --fps.
        Source::Fake { address } => Box::new(FakeCamera::connect(address)?),
        Source::Spinnaker { serial, library } => Box::new(SpinnakerCamera::open(
            blank_as_none(serial).as_deref(),
            settings.frame_rate,
            blank_as_none(library).as_deref(),
        )?),
    })
}
