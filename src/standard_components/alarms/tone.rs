//! The pips the Alarms viewer can sound as an alarm goes off: a couple, at a pitch for each alarm, from the default
//! output device. It's on a thread of its own, started the first time it's needed, as finding a device takes a while
//! and passes can't wait. Alarms going off together pip one after another. A machine with no sound just stays quiet,
//! with a warning.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Each alarm's pitch, by `Source`.
const PITCHES_HZ: [f32; 4] = [880.0, 660.0, 1100.0, 1320.0];
const VOLUME: f32 = 0.2;
/// Seconds a pip lasts, and the gap after each.
const PIP: f32 = 0.1;
const GAP: f32 = 0.08;
/// Seconds two pips and their gaps take.
const PIPS: f32 = 2.0 * (PIP + GAP);

/// The pips at `pitch`, `seconds` in: each swelling and fading, so as not to click.
fn wave(pitch: f32, seconds: f32) -> f32 {
    let within = seconds % (PIP + GAP);
    if within < PIP { VOLUME * (PI * within / PIP).sin() * (TAU * pitch * seconds).sin() } else { 0.0 }
}

/// Which alarms have pips waiting, and whether to stop, for the thread.
#[derive(Default)]
struct Flags {
    waiting: [AtomicBool; 4],
    stop: AtomicBool,
}

/// The sound, which stops when this is dropped.
#[derive(Default)]
pub struct Tone {
    flags: Arc<Flags>,
    thread: Option<JoinHandle<()>>,
}

impl Tone {
    /// Sounds alarm `index`'s pips, without waiting.
    pub fn pips(&mut self, index: usize) {
        self.flags.waiting[index].store(true, Ordering::Relaxed);
        if self.thread.is_none() {
            let flags = self.flags.clone();
            self.thread = std::thread::Builder::new().name("Alarm tone".into()).spawn(move || play(&flags)).ok();
        }
    }
}

impl Drop for Tone {
    fn drop(&mut self) {
        self.flags.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }
}

/// Opens the default device and plays until told to stop.
fn play(flags: &Arc<Flags>) {
    let stream = match open(flags.clone()) {
        Ok(stream) => stream,
        Err(error) => return log::warn!("Can't sound the alarms: {error}"),
    };
    while !flags.stop.load(Ordering::Relaxed) {
        std::thread::park();
    }
    drop(stream);
}

/// What's sounding: its pitch, and how many samples in.
type Sounding = Option<(f32, u32)>;

/// The next sample, moving on to whatever's waiting once what's sounding has finished.
fn next(sounding: &mut Sounding, flags: &Flags, rate: f32) -> f32 {
    if sounding.is_none() {
        let waiting = flags.waiting.iter().position(|waiting| waiting.swap(false, Ordering::Relaxed));
        *sounding = waiting.map(|index| (PITCHES_HZ[index], 0));
    }
    let Some((pitch, sample)) = sounding else { return 0.0 };
    let seconds = *sample as f32 / rate;
    *sample += 1;
    let value = wave(*pitch, seconds);
    if seconds >= PIPS {
        *sounding = None;
    }
    value
}

fn open(flags: Arc<Flags>) -> Result<cpal::Stream, String> {
    let device = cpal::default_host().default_output_device().ok_or("no output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    if config.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!("its samples are {}, not floats", config.sample_format()));
    }
    let config = config.config();
    let (channels, rate) = (config.channels as usize, config.sample_rate as f32);
    let mut sounding = None;
    let write = move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
        for frame in output.chunks_mut(channels.max(1)) {
            frame.fill(next(&mut sounding, &flags, rate));
        }
    };
    let error = |error| log::warn!("The alarms' tone: {error}");
    let stream = device.build_output_stream(config, write, error, None).map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    #[test]
    fn two_pips_then_quiet_and_each_alarm_in_turn() {
        let flags = Flags::default();
        flags.waiting[1].store(true, Ordering::Relaxed);
        flags.waiting[3].store(true, Ordering::Relaxed);
        let mut sounding = None;
        let samples: Vec<f32> = (0..(RATE * PIPS * 3.0) as usize).map(|_| next(&mut sounding, &flags, RATE)).collect();
        // How many times each stretch rises through zero, for its pitch; and how many runs of sound there are.
        let pitch = |from: f32| {
            let at = |seconds: f32| (seconds * RATE) as usize;
            let part = &samples[at(from)..at(from + PIP)];
            part.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() as f32 / PIP
        };
        assert!((pitch(0.0) - PITCHES_HZ[1]).abs() < 30.0, "{}", pitch(0.0));
        assert!((pitch(PIPS) - PITCHES_HZ[3]).abs() < 30.0, "{}", pitch(PIPS));
        let quiet = |from: f32, to: f32| {
            samples[(from * RATE) as usize + 2..(to * RATE) as usize - 2].iter().all(|x| *x == 0.0)
        };
        assert!(quiet(PIP, PIP + GAP), "a gap between the pips");
        assert!(quiet(2.0 * PIPS + 0.01, 3.0 * PIPS), "quiet after");
    }

    #[test]
    fn the_pitches_differ() {
        let mut pitches = PITCHES_HZ.map(|pitch| pitch as u32);
        pitches.sort();
        assert!(pitches.windows(2).all(|pair| pair[0] != pair[1]));
    }

    /// Opens the sound device, silently: not all machines have one.
    #[test]
    #[ignore]
    fn the_device_opens() {
        let _stream = open(Arc::default()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}
