//! The tone the Alarms viewer can sound while an alarm's going off: beeps from the default output device. It's on a
//! thread of its own, started the first time it's needed, as finding a device takes a while and passes can't wait. A
//! machine with no sound just stays quiet, with a warning.

use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const PITCH_HZ: f32 = 880.0;
const VOLUME: f32 = 0.2;
/// Seconds each beep lasts, and the gap after it.
const BEEP: f32 = 0.25;

/// The tone at `seconds` in: beeps, the same length as the gaps between them.
fn wave(seconds: f32) -> f32 {
    if seconds % (2.0 * BEEP) < BEEP { VOLUME * (TAU * PITCH_HZ * seconds).sin() } else { 0.0 }
}

/// Whether it should sound, and whether to stop, for the thread.
#[derive(Default)]
struct Flags {
    sounding: AtomicBool,
    stop: AtomicBool,
}

/// The tone, which stops when this is dropped.
#[derive(Default)]
pub struct Tone {
    flags: Arc<Flags>,
    thread: Option<JoinHandle<()>>,
}

impl Tone {
    /// Starts or stops the sound, without waiting.
    pub fn set(&mut self, sounding: bool) {
        self.flags.sounding.store(sounding, Ordering::Relaxed);
        if sounding && self.thread.is_none() {
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

/// Opens the default device and plays until told to stop: silence, unless it's sounding.
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

fn open(flags: Arc<Flags>) -> Result<cpal::Stream, String> {
    let device = cpal::default_host().default_output_device().ok_or("no output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    if config.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!("its samples are {}, not floats", config.sample_format()));
    }
    let config = config.config();
    let (channels, rate) = (config.channels as usize, config.sample_rate as f32);
    let mut sample = 0u64;
    let write = move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
        let sounding = flags.sounding.load(Ordering::Relaxed);
        for frame in output.chunks_mut(channels.max(1)) {
            // From the start of a beep each time it sounds, wrapping before a float's too coarse to time it.
            sample = if sounding { (sample + 1) % (rate as u64 * 60) } else { 0 };
            frame.fill(if sounding { wave(sample as f32 / rate) } else { 0.0 });
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

    #[test]
    fn it_beeps_with_gaps() {
        let peak = |from: f32, to: f32| {
            (0..1000).map(|i| wave(from + (to - from) * i as f32 / 1000.0).abs()).fold(0.0, f32::max)
        };
        assert!((peak(0.0, BEEP) - VOLUME).abs() < 0.01, "a beep");
        assert_eq!(peak(BEEP + 0.001, 2.0 * BEEP - 0.001), 0.0, "a gap");
    }

    /// Opens the sound device, silently: not all machines have one.
    #[test]
    #[ignore]
    fn the_device_opens() {
        let _stream = open(Arc::default()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}
