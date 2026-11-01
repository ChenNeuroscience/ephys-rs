//! Acquisition boards: the Acquisition Board node (`node`), acquiring on a thread of its own (`capture`), and
//! the backends it acquires from, after the Open Ephys acquisition board plugins: the fake board in
//! synthetic_streams/ (`fake`), boards on Opal Kelly FPGAs running Intan's Rhythm firmware (the Open Ephys
//! Acquisition Board before generation 3, and Intan's RHD USB Interface Board), and the generation 3 Open
//! Ephys board, over ONI.
//!
//! Every board has headstages of Intan RHD chips on its ports, each with 3 auxiliary inputs (accelerometers,
//! say), and most have analogue (ADC) and digital (TTL) inputs of their own. All are sampled together.

pub mod capture;
mod fake;
pub mod node;

use std::io;

use serde::{Deserialize, Serialize};

pub use fake::FakeBoard;
#[cfg(test)]
pub use fake::serve_test_board;

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

/// A headstage on one of a board's ports (A, B, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Headstage {
    pub port: char,
    pub channels: u32,
}

impl Headstage {
    /// The headstages' channels, all together.
    pub fn total(headstages: &[Headstage]) -> u32 {
        headstages.iter().map(|h| h.channels).sum()
    }

    /// "A: 32, B: 64".
    pub fn describe(headstages: &[Headstage]) -> String {
        if headstages.is_empty() {
            return "none".into();
        }
        let each: Vec<String> = headstages.iter().map(|h| format!("{}: {}", h.port, h.channels)).collect();
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
}

/// Samples from a board, channel by channel: for each kind of input, `samples` values for each channel in
/// turn, scaled (µV for the headstages, V for the auxiliary and analogue inputs). Reused from block to block.
#[derive(Clone, Debug, Default)]
pub struct Block {
    pub samples: usize,
    pub ephys: Vec<f32>,
    pub aux: Vec<f32>,
    pub adc: Vec<f32>,
    /// The digital inputs, a bit per line, for each sample.
    pub ttl: Vec<u32>,
    /// The board's sample number of the first sample, counting from the start.
    pub first: u64,
}

impl Block {
    /// Makes room for `samples` of each of the given channels (keeping the memory there is).
    pub fn resize(&mut self, samples: usize, ephys: usize, aux: usize, adc: usize) {
        self.samples = samples;
        self.ephys.resize(samples * ephys, 0.0);
        self.aux.resize(samples * aux, 0.0);
        self.adc.resize(samples * adc, 0.0);
        self.ttl.resize(samples, 0);
    }

    /// How many channels of a kind of input (one of the block's) it has.
    pub fn channels(&self, values: &[f32]) -> usize {
        values.len().checked_div(self.samples).unwrap_or(0)
    }
}

/// A board acquiring.
pub trait Board: Send {
    fn info(&self) -> &BoardInfo;

    /// Starts acquiring at `config`.
    fn start(&mut self, config: &Config) -> io::Result<()>;

    /// Waits briefly for samples, filling `block` with those that came. `Ok(false)` means none came yet; the
    /// caller should check whether it has been asked to stop and then call again.
    fn read(&mut self, block: &mut Block) -> io::Result<bool>;

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
}

impl Default for Source {
    fn default() -> Self {
        Source::Fake { address: FAKE_ADDRESS.into() }
    }
}

/// Where `fake_acquisition_board.py` serves samples by default.
pub const FAKE_ADDRESS: &str = "127.0.0.1:27031";

/// Opens the board, finding out what it has.
pub fn open(source: &Source) -> io::Result<Box<dyn Board>> {
    Ok(match source {
        Source::Fake { address } => Box::new(FakeBoard::connect(address)?),
    })
}
