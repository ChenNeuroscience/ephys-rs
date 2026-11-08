//! Acquisition boards: the Acquisition Board node (`node`), acquiring on a thread of its own (`capture`), and
//! the backends it acquires from, after the Open Ephys acquisition board plugins: the fake board in
//! synthetic_streams/ (`fake`), and boards running Intan's Rhythm firmware (`rhythm`): on Opal Kelly FPGAs (the
//! Open Ephys Acquisition Board before generation 3, and Intan's RHD USB Interface Board: `opal_kelly`), and
//! the generation 3 Open Ephys board, over ONI (`oni`). The RHD2000 chips themselves are in `rhd`.
//!
//! Every board has headstages of Intan RHD chips on its ports, each with 3 auxiliary inputs (accelerometers,
//! say), and most have analogue (ADC) and digital (TTL) inputs of their own. All are sampled together.

pub mod capture;
mod fake;
mod liboni;
pub mod node;
mod oni;
pub mod opal_kelly;
mod rhd;
mod rhythm;

use std::io;

use serde::{Deserialize, Serialize};

use crate::Edges;
use crate::core::engine::{DataStreamType, Node, PASS_DURATION};
use crate::standard_components::ephys::{Continuous, ContinuousBuffer, Events, EventsBuffer};

pub use fake::FakeBoard;
#[cfg(test)]
pub use fake::serve_test_board;
pub use oni::OniSettings;
pub use opal_kelly::{OpalKellyKind, OpalKellySettings};

/// Microvolts per bit of headstage data.
pub const EPHYS_MICROVOLTS_PER_BIT: f32 = 0.195;
/// Volts per bit of auxiliary input.
pub const AUX_VOLTS_PER_BIT: f32 = 0.0000374;
/// Volts per bit of the board's analogue inputs.
pub const ADC_VOLTS_PER_BIT: f32 = 0.000_152_587_89;
/// Auxiliary inputs per headstage.
pub const AUX_PER_HEADSTAGE: u32 = 3;

/// The sample rates Rhythm boards can run at, in Hz.
pub const SAMPLE_RATES: [u32; 17] =
    [1000, 1250, 1500, 2000, 2500, 3000, 3333, 4000, 5000, 6250, 8000, 10000, 12500, 15000, 20000, 25000, 30000];

/// The data sources headstages can be on, in boards' order: each port's (A to H) two MISO lines.
pub const PORTS: [&str; 16] =
    ["A1", "A2", "B1", "B2", "C1", "C2", "D1", "D2", "E1", "E2", "F1", "F2", "G1", "G2", "H1", "H2"];

/// A headstage on one of a board's data sources ("A1", "B1"...).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Headstage {
    pub port: String,
    pub channels: u32,
}

impl Headstage {
    /// The headstages' channels, all together.
    pub fn total(headstages: &[Headstage]) -> u32 {
        headstages.iter().map(|h| h.channels).sum()
    }

    /// "A1 32, B1 64".
    pub fn describe(headstages: &[Headstage]) -> String {
        if headstages.is_empty() {
            return "none".into();
        }
        let each: Vec<String> = headstages.iter().map(|h| format!("{} {}", h.port, h.channels)).collect();
        each.join(", ")
    }
}

/// What a board is and has.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoardInfo {
    pub description: String,
    pub headstages: Vec<Headstage>,
    pub adc_channels: u32,
    pub ttl_lines: u32,
}

/// The analogue filters on the headstages' chips, and the chips' DSP high-pass filter (which takes off any
/// offset).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bandwidth {
    pub lower_hz: f64,
    pub upper_hz: f64,
    /// The DSP filter's cut-off, if it's on.
    pub dsp_hz: Option<f64>,
}

impl Default for Bandwidth {
    fn default() -> Self {
        Bandwidth { lower_hz: 1.0, upper_hz: 7500.0, dsp_hz: Some(0.5) }
    }
}

/// How to run a board.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub sample_rate: u32,
    pub bandwidth: Bandwidth,
    /// The headstages wanted (a board that can read them as set does; otherwise it reads those it has).
    pub headstages: Vec<Headstage>,
}

/// A pass's worth of samples from a board, in the buffers passes take them in (see `capture`): the headstages',
/// auxiliary and analogue inputs' (µV for the headstages, V for the rest), each channel's from the start of its
/// row, and where the digital inputs change. The capture thread fills them, a read at a time, from `len()` on.
pub struct Samples {
    pub ephys: ContinuousBuffer,
    pub aux: ContinuousBuffer,
    pub adc: ContinuousBuffer,
    pub events: EventsBuffer,
}

impl Samples {
    /// Empty buffers for one pass of these channels.
    pub fn new(ephys: Continuous, aux: Continuous, adc: Continuous, events: Events) -> Self {
        Samples {
            ephys: ephys.buffer(PASS_DURATION),
            aux: aux.buffer(PASS_DURATION),
            adc: adc.buffer(PASS_DURATION),
            events: events.buffer(PASS_DURATION),
        }
    }

    /// The memory they take.
    pub fn bytes(&self) -> usize {
        let continuous = [&self.ephys, &self.aux, &self.adc].map(|b| b.data.capacity() * size_of::<f32>());
        continuous.iter().sum::<usize>()
            + self.events.events.capacity() * size_of::<crate::standard_components::ephys::Event>()
    }

    /// Samples so far.
    pub fn len(&self) -> usize {
        self.ephys.len
    }

    /// Whether there are no samples yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Samples there's room for.
    pub fn room(&self) -> usize {
        self.ephys.capacity - self.ephys.len
    }

    /// Empties them, to fill again.
    pub fn clear(&mut self) {
        (self.ephys.len, self.aux.len, self.adc.len) = (0, 0, 0);
        self.events.events.clear();
        self.events.pass_samples = 0;
    }

    /// The channels of each kind: headstage, auxiliary and analogue.
    #[cfg(test)]
    pub fn layout(&self) -> (usize, usize, usize) {
        (self.ephys.channels, self.aux.channels, self.adc.channels)
    }

    /// Adds `count` samples, which a board's just written after those there were.
    fn grow(&mut self, count: usize) {
        for buffer in [&mut self.ephys, &mut self.aux, &mut self.adc] {
            buffer.len += count;
        }
        self.events.pass_samples += count;
    }
}

/// What a board decoded: how many samples, and the board's number for the first (counting from the start).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decoded {
    pub samples: usize,
    pub first: u64,
}

/// A board acquiring.
pub trait Board: Send {
    fn info(&self) -> &BoardInfo;

    /// Starts acquiring at `config`.
    fn start(&mut self, config: &Config) -> io::Result<()>;

    /// The channels of each kind it's acquiring, once started: headstage, auxiliary and analogue.
    fn layout(&self) -> (usize, usize, usize);

    /// Waits briefly for samples, keeping them to decode. `Ok(false)` means none came yet; the caller should check
    /// whether it has been asked to stop and then call again.
    fn fetch(&mut self) -> io::Result<bool>;

    /// Decodes as many of the samples fetched as there's room for in `into` (laid out as `layout` says), after
    /// those there, with each sample's digital inputs (a bit per line) in `ttl`. Quick: it's done while a pass might
    /// be waiting to take them. Whatever didn't fit is kept for next time.
    fn decode(&mut self, into: &mut Samples, ttl: &mut Vec<u32>) -> io::Result<Decoded>;

    /// Problems setting the board up that don't stop it working.
    fn warnings(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Which board to acquire from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// Samples streamed over TCP by `synthetic_streams/fake_acquisition_board.py`.
    Fake { address: String },
    /// A board on an Opal Kelly FPGA running Rhythm firmware.
    OpalKelly { board: OpalKellyKind },
    /// The generation 3 Open Ephys Acquisition Board.
    Oni,
}

impl Source {
    /// The board's digital inputs.
    pub fn ttl_lines(&self) -> u32 {
        match self {
            Source::Fake { .. } => 8,
            Source::OpalKelly { board, .. } => board.ttl_lines(),
            Source::Oni => 8,
        }
    }
}

impl Default for Source {
    fn default() -> Self {
        Source::Fake { address: FAKE_ADDRESS.into() }
    }
}

/// Where `fake_acquisition_board.py` serves samples by default.
pub const FAKE_ADDRESS: &str = "127.0.0.1:27031";

/// Opens the board, finding out what it has. Where its libraries are is as the global settings say now, so a change
/// applies when it's next tried.
pub fn open(source: &Source) -> io::Result<Box<dyn Board>> {
    let (opal_kelly, oni) = <node::AcquisitionBoard as Node<Edges>>::global_settings();
    Ok(match source {
        Source::Fake { address } => Box::new(FakeBoard::connect(address)?),
        Source::OpalKelly { board } => Box::new(opal_kelly::open(*board, &opal_kelly)?),
        Source::Oni => Box::new(oni::open(&oni)?),
    })
}
