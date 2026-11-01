//! Boards running Intan's Rhythm firmware, whatever carries their data: setting the chips up, finding the
//! headstages and the best cable delays, and reading samples. Ported from the Open Ephys acquisition board
//! plugins (`AcqBoardOpalKelly`, `AcqBoardONI`). The transports (`opal_kelly`, `oni`) do the talking.
//!
//! Each sample from the firmware is 16-bit words, little-endian: a 4-word header (a magic number), a 2-word
//! sample counter, then for each of the 3 command slots each enabled stream's result, then each stream's 32
//! amplifier channels (interleaved stream by stream), a filler word per stream, 8 analogue inputs, and the
//! digital inputs and outputs.

use std::io;

use super::rhd::{self, CHIP_RHD2132, CHIP_RHD2164, CHIP_RHD2216, REGISTER_59_MISO_A, RHD2132_16CH_OFFSET, Registers};
use super::{AUX_VOLTS_PER_BIT, Bandwidth, Block, Board, BoardInfo, Config, EPHYS_MICROVOLTS_PER_BIT, Headstage};

const HEADER_MAGIC: u64 = 0xc691_1999_2702_1942;
/// The data sources, in the firmware's order: each port's two MISO lines.
const SOURCES: [&str; 8] = ["A1", "A2", "B1", "B2", "C1", "C2", "D1", "D2"];
/// The second data stream of an RHD2164 (its MISO B), read with double data rate: its source + 8.
const DDR: u32 = 8;
/// Samples run while scanning: enough for the register config command list's answers.
const SCAN_SAMPLES: u32 = 64;

/// Bytes in a sample with `streams` data streams enabled.
pub fn sample_bytes(streams: u32) -> usize {
    2 * (4 + 2 + 36 * streams as usize + 8 + 2)
}

/// What carries a Rhythm board's commands and data.
pub trait Transport: Send {
    fn description(&self) -> String;
    fn max_streams(&self) -> u32;
    fn ttl_lines(&self) -> u32;
    /// Volts from an analogue input's reading.
    fn adc_volts(&self, raw: u16) -> f32;
    /// Clock steps added to the cable delay worked out from a cable's length.
    fn extra_delay(&self) -> u32 {
        0
    }

    fn set_sample_rate(&mut self, rate: u32) -> io::Result<()>;
    fn upload_command_list(&mut self, list: &[u32], slot: usize, bank: u32);
    /// Selects a command list bank for a slot, on every port.
    fn select_aux_bank(&mut self, slot: usize, bank: u32);
    fn select_aux_length(&mut self, slot: usize, end: u32);
    fn set_cable_delay(&mut self, port: usize, delay: u32);
    fn set_data_source(&mut self, stream: u32, source: u32);
    /// Enables the first `count` data streams, and no others.
    fn enable_streams(&mut self, count: u32);
    fn set_continuous(&mut self, on: bool);
    fn set_max_time_step(&mut self, steps: u32);

    /// Runs `samples` samples, and reads them into `buf` (one after another).
    fn run_once(&mut self, samples: u32, buf: &mut Vec<u8>) -> io::Result<()>;
    /// Starts running continuously.
    fn start(&mut self) -> io::Result<()>;
    /// Reads samples that are ready into `buf` (one after another), returning how many; none if none came.
    fn read(&mut self, buf: &mut Vec<u8>) -> io::Result<usize>;
    fn stop(&mut self);
    /// Shows on the board's LEDs whether it's running.
    fn leds(&mut self, running: bool);
}

/// A data stream the board reads: from which source, its chip, and how many of its channels are taken.
#[derive(Clone, Copy, Debug)]
struct Stream {
    source: u32,
    /// The second stream of an RHD2164 (which has no auxiliary inputs of its own).
    second: bool,
    chip: i32,
    channels: usize,
}

pub struct RhythmBoard<T: Transport> {
    transport: T,
    info: BoardInfo,
    /// The chip found on each data source, by scanning.
    chips: [Option<i32>; 8],
    streams: Vec<Stream>,
    enabled_streams: u32,
    registers: Registers,
    sample_rate: u32,
    buf: Vec<u8>,
    /// Each stream's auxiliary inputs as last sampled (each is sampled every fourth sample, then held).
    aux: Vec<[f32; 3]>,
    aux_due: Vec<[f32; 3]>,
    /// The sample counter's start, and where the samples so far have taken it (counting through its wraps).
    sample: u64,
    warnings: Vec<String>,
    running: bool,
}

impl<T: Transport> RhythmBoard<T> {
    /// Sets up a board (its FPGA already configured), calibrates its chips' ADCs and scans for headstages.
    pub fn new(transport: T) -> io::Result<Self> {
        let mut board = RhythmBoard {
            transport,
            info: BoardInfo::default(),
            chips: [None; 8],
            streams: Vec::new(),
            enabled_streams: 0,
            registers: Registers::new(30000.0),
            sample_rate: 30000,
            buf: Vec::new(),
            aux: Vec::new(),
            aux_due: Vec::new(),
            sample: 0,
            warnings: Vec::new(),
            running: false,
        };
        let t = &mut board.transport;
        for slot in 0..3 {
            t.select_aux_bank(slot, 0);
            t.select_aux_length(slot, 0);
        }
        t.set_continuous(true);
        t.set_max_time_step(u32::MAX);
        // Each stream reads its port's first MISO line to begin with.
        for (stream, source) in [0, 2, 4, 6, 1, 3, 5, 7].into_iter().enumerate() {
            t.set_data_source(stream as u32, source);
            if t.max_streams() > 8 {
                t.set_data_source(stream as u32 + 8, source);
            }
        }
        board.enable_streams(1);
        board.set_sample_rate(30000)?;
        for port in 0..4 {
            board.set_cable_length(port, 0.914);
        }
        board.upload_registers(&Bandwidth::default());
        // Calibrate the chips' ADCs.
        board.transport.select_aux_bank(2, 0);
        board.transport.run_once(SCAN_SAMPLES, &mut board.buf)?;
        board.transport.select_aux_bank(2, 1);
        board.scan()?;
        board.info = BoardInfo {
            description: board.transport.description(),
            headstages: board.found_headstages(),
            adc_channels: 8,
            ttl_lines: board.transport.ttl_lines(),
        };
        board.transport.leds(false);
        Ok(board)
    }

    fn enable_streams(&mut self, count: u32) {
        self.transport.enable_streams(count);
        self.enabled_streams = count;
    }

    fn set_sample_rate(&mut self, rate: u32) -> io::Result<()> {
        self.transport.set_sample_rate(rate)?;
        self.sample_rate = rate;
        self.registers.set_sample_rate(rate as f64);
        Ok(())
    }

    /// The cable delay for a cable `meters` long, at the sample rate.
    fn set_cable_length(&mut self, port: usize, meters: f64) {
        // The data clock sampling MISO runs at 2800 × the sample rate.
        let t_step = 1.0 / (2800.0 * self.sample_rate as f64);
        let delay = 2.0 * meters / (0.555 * 299_792_458.0) + 1.9e-9 + 9.0e-9 + 1.4e-9 + 6.7e-9;
        let steps = ((delay / t_step + 1.0) + 0.5).floor().max(1.0) as u32;
        self.transport.set_cable_delay(port, (steps + self.transport.extra_delay()).min(15));
    }

    /// Uploads the command lists for the sample rate, bandwidth and DSP: the third slot's bank 0 calibrates,
    /// bank 1 runs normally, bank 2 with fast settle. Returns the bandwidth set (lower, upper and DSP).
    fn upload_registers(&mut self, bandwidth: &Bandwidth) -> (f64, f64, Option<f64>) {
        let t = &mut self.transport;
        self.registers.set_dig_out_low();
        let list = self.registers.update_dig_out();
        t.upload_command_list(&list, 0, 0);
        t.select_aux_length(0, list.len() as u32 - 1);
        t.select_aux_bank(0, 0);
        let list = self.registers.temperature_sensor();
        t.upload_command_list(&list, 1, 0);
        t.select_aux_length(1, list.len() as u32 - 1);
        t.select_aux_bank(1, 0);

        let dsp = bandwidth.dsp_hz.map(|hz| self.registers.set_dsp_cutoff(hz));
        self.registers.enable_dsp(dsp.is_some());
        let lower = self.registers.set_lower_bandwidth(bandwidth.lower_hz);
        let upper = self.registers.set_upper_bandwidth(bandwidth.upper_hz);
        self.registers.enable_aux(true);
        self.registers.set_fast_settle(false);
        t.upload_command_list(&self.registers.register_config(true), 2, 0);
        let list = self.registers.register_config(false);
        t.upload_command_list(&list, 2, 1);
        t.select_aux_length(2, list.len() as u32 - 1);
        self.registers.set_fast_settle(true);
        t.upload_command_list(&self.registers.register_config(false), 2, 2);
        self.registers.set_fast_settle(false);
        t.select_aux_bank(2, 1);
        (lower, upper, dsp)
    }

    /// Word `word` (counting from the sample's start) of sample `t` in `buf`.
    fn word(&self, t: usize, word: usize) -> u16 {
        let at = sample_bytes(self.enabled_streams) * t + 2 * word;
        u16::from_le_bytes([self.buf[at], self.buf[at + 1]])
    }

    /// Word offset, within a sample, of command slot `slot`'s result for stream `stream`.
    fn aux_word(&self, slot: usize, stream: usize) -> usize {
        6 + slot * self.enabled_streams as usize + stream
    }

    /// Finds which chip is on each data source, and each port's best cable delay: at each delay, reads every
    /// source's chip ID back; the delays at which a chip answers are good, and the second good one is best (the
    /// first if there are only one or two).
    fn scan(&mut self) -> io::Result<()> {
        for source in 0..8 {
            self.transport.set_data_source(source, source);
        }
        self.enable_streams(8);
        self.transport.select_aux_bank(2, 0);
        let mut good: [Vec<u32>; 8] = Default::default();
        let mut chips = [None; 8];
        for delay in 0..16 {
            for port in 0..4 {
                self.transport.set_cable_delay(port, delay);
            }
            self.transport.run_once(SCAN_SAMPLES, &mut self.buf)?;
            for (source, good) in good.iter_mut().enumerate() {
                let results: Vec<u16> = (0..60).map(|t| self.word(t, self.aux_word(2, source))).collect();
                let Some((id, register_59)) = rhd::chip_id(&results) else { continue };
                if id == CHIP_RHD2132 || id == CHIP_RHD2216 || (id == CHIP_RHD2164 && register_59 == REGISTER_59_MISO_A)
                {
                    good.push(delay);
                    chips[source] = Some(id);
                }
            }
        }
        self.chips = chips;
        for port in 0..4 {
            let best = |source: usize| match good[source].len() {
                0 => 0,
                1 | 2 => good[source][0],
                _ => good[source][1],
            };
            self.transport.set_cable_delay(port, best(2 * port).max(best(2 * port + 1)));
        }
        self.transport.select_aux_bank(2, 1);
        Ok(())
    }

    /// The headstages scanning found, at their full channel counts.
    fn found_headstages(&self) -> Vec<Headstage> {
        let mut headstages = Vec::new();
        for (source, chip) in self.chips.iter().enumerate() {
            let channels = match chip {
                Some(CHIP_RHD2164) => 64,
                Some(CHIP_RHD2132) => 32,
                Some(CHIP_RHD2216) => 16,
                _ => continue,
            };
            headstages.push(Headstage { port: SOURCES[source].into(), channels });
        }
        headstages
    }

    /// The data streams to read for `wanted` headstages, if the board has them (an RHD2132 can be read as 16
    /// channels, an RHD2164 as 32); otherwise for every headstage found, at full channel counts.
    fn choose_streams(&self, wanted: &[Headstage]) -> (Vec<Stream>, Option<String>) {
        let max = self.transport.max_streams() as usize;
        let streams_for = |headstages: &[Headstage]| -> Option<Vec<Stream>> {
            let mut streams = Vec::new();
            for headstage in headstages {
                let source = SOURCES.iter().position(|s| *s == headstage.port)? as u32;
                let chip = self.chips[source as usize]?;
                let stream =
                    |second, channels| Stream { source: source + if second { DDR } else { 0 }, second, chip, channels };
                match (chip, headstage.channels) {
                    (CHIP_RHD2164, 64) => streams.extend([stream(false, 32), stream(true, 32)]),
                    (CHIP_RHD2164, 32) | (CHIP_RHD2132, 32) => streams.push(stream(false, 32)),
                    (CHIP_RHD2132, 16) | (CHIP_RHD2216, 16) => streams.push(stream(false, 16)),
                    _ => return None,
                }
            }
            (streams.len() <= max).then_some(streams)
        };
        match streams_for(wanted) {
            Some(streams) => (streams, None),
            None => {
                let found = self.found_headstages();
                let mut streams = streams_for(&found).unwrap_or_default();
                streams.truncate(max);
                let note = format!(
                    "The board's headstages are {}, so it's acquiring all those rather than the {} set",
                    Headstage::describe(&found),
                    Headstage::describe(wanted)
                );
                (streams, Some(note))
            }
        }
    }

    fn stop(&mut self) {
        if self.running {
            self.transport.stop();
            self.transport.leds(false);
            self.running = false;
        }
    }
}

impl<T: Transport> Drop for RhythmBoard<T> {
    fn drop(&mut self) {
        self.stop();
    }
}

impl<T: Transport> Board for RhythmBoard<T> {
    fn info(&self) -> &BoardInfo {
        &self.info
    }

    fn start(&mut self, config: &Config) -> io::Result<()> {
        let (streams, note) = self.choose_streams(&config.headstages);
        self.warnings.clear();
        self.warnings.extend(note);
        self.set_sample_rate(config.sample_rate)?;
        // The best cable delays depend on the sample rate.
        if config.sample_rate != 30000 {
            self.scan()?;
        }
        for (stream, s) in streams.iter().enumerate() {
            self.transport.set_data_source(stream as u32, s.source);
        }
        self.enable_streams(streams.len() as u32);
        self.streams = streams;
        let (lower, upper, dsp) = self.upload_registers(&config.bandwidth);
        let near = |got: f64, asked: f64| (got - asked).abs() <= 0.05 * asked;
        if !near(lower, config.bandwidth.lower_hz) || !near(upper, config.bandwidth.upper_hz) {
            self.warnings.push(format!("The chips' bandwidth is {lower:.2} to {upper:.0} Hz, as near as they get"));
        }
        if let (Some(dsp), Some(asked)) = (dsp, config.bandwidth.dsp_hz)
            && !near(dsp, asked)
        {
            self.warnings.push(format!("The DSP high-pass is at {dsp:.2} Hz, as near as it gets"));
        }
        self.aux = vec![[0.0; 3]; self.streams.len()];
        self.aux_due = vec![[0.0; 3]; self.streams.len()];
        self.sample = 0;
        self.transport.leds(true);
        self.transport.start()?;
        self.running = true;
        Ok(())
    }

    fn read(&mut self, block: &mut Block) -> io::Result<bool> {
        let samples = self.transport.read(&mut self.buf)?;
        if samples == 0 {
            return Ok(false);
        }
        let streams = &self.streams;
        let ephys: usize = streams.iter().map(|s| s.channels).sum();
        let aux_streams: Vec<usize> = (0..streams.len()).filter(|&s| !streams[s].second).collect();
        block.resize(samples, ephys, 3 * aux_streams.len(), 8);
        let n = self.enabled_streams as usize;
        let size = sample_bytes(self.enabled_streams);
        for t in 0..samples {
            let at = size * t;
            let header = u64::from_le_bytes(self.buf[at..at + 8].try_into().unwrap());
            if header != HEADER_MAGIC {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "lost track of the board's samples"));
            }
            if t == 0 {
                // The firmware's 32-bit sample counter: carry on from where the samples so far left off.
                let counter = u32::from_le_bytes(self.buf[at + 8..at + 12].try_into().unwrap()) as u64;
                let wraps = self.sample >> 32;
                let mut first = (wraps << 32) | counter;
                if first + (1 << 31) < self.sample {
                    first += 1 << 32;
                }
                block.first = first;
            }
            // Headstage channels: 32 per stream, interleaved stream by stream.
            let mut channel = 0;
            for (stream, s) in self.streams.iter().enumerate() {
                let offset = if s.chip == CHIP_RHD2132 && s.channels == 16 { RHD2132_16CH_OFFSET } else { 0 };
                for c in 0..s.channels {
                    let raw = self.word(t, 6 + 3 * n + (offset + c) * n + stream);
                    block.ephys[channel * samples + t] = (raw as f32 - 32768.0) * EPHYS_MICROVOLTS_PER_BIT;
                    channel += 1;
                }
            }
            // Auxiliary inputs, from the second command slot: each sampled every fourth sample (counting from the
            // start, as the command list does), and held.
            let which = ((block.first + t as u64 + 3) % 4) as usize;
            for (i, &stream) in aux_streams.iter().enumerate() {
                if which < 3 {
                    let raw = self.word(t, self.aux_word(1, stream));
                    self.aux_due[stream][which] = (raw as f32 - 32768.0) * AUX_VOLTS_PER_BIT;
                } else {
                    self.aux[stream] = self.aux_due[stream];
                }
                for input in 0..3 {
                    block.aux[(3 * i + input) * samples + t] = self.aux[stream][input];
                }
            }
            let adc = 6 + 36 * n;
            for input in 0..8 {
                block.adc[input * samples + t] = self.transport.adc_volts(self.word(t, adc + input));
            }
            block.ttl[t] = self.word(t, adc + 8) as u32;
        }
        self.sample = block.first + samples as u64;
        Ok(true)
    }

    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_as_long_as_intans() {
        // 4 header + 2 counter + 36 per stream + 8 analogue + 2 digital words.
        assert_eq!(sample_bytes(1), 2 * 52);
        assert_eq!(sample_bytes(8), 2 * 304);
    }
}
