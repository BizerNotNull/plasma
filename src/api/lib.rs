//! Shared monophonic synth controls and real system audio.
//!
//! Control callers may block each other briefly. The audio callback owns its
//! kernel bank and only tries the control mutex once per buffer: on contention
//! it keeps the previous fixed-size snapshot. It never allocates or waits.
//! Note commands are latest-state controls, not an event queue; commands between
//! audio buffers can coalesce. Every observed note-on retriggers kernel phases.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use plasma_kernel::{OSCILLATOR_COUNT, OscillatorBank};
pub use plasma_kernel::{OscillatorParams, Waveform};

const HEADROOM: f32 = 0.8 / OSCILLATOR_COUNT as f32;
const RAMP_SECONDS: f32 = 0.005;

#[derive(Clone, Copy)]
struct Controls {
    oscillators: [OscillatorParams; OSCILLATOR_COUNT],
    volume: f32,
    note: Option<u8>,
    generation: u64,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            oscillators: std::array::from_fn(|index| OscillatorParams {
                level: if index == 0 { 1.0 } else { 0.0 },
                ..Default::default()
            }),
            volume: 0.25,
            note: None,
            generation: 0,
        }
    }
}

/// Cloneable control handle. Clones address the same synth state.
#[derive(Clone, Default)]
pub struct Synth {
    controls: Arc<Mutex<Controls>>,
}

impl Synth {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn params(&self, index: usize) -> Result<OscillatorParams, String> {
        self.controls
            .lock()
            .map_err(|_| "Synth control lock was poisoned".to_owned())?
            .oscillators
            .get(index)
            .copied()
            .ok_or_else(|| plasma_kernel::Error::InvalidOscillatorIndex.to_string())
    }

    /// Invalid updates leave shared state unchanged. Kernel validation is the
    /// single source of truth, including finite-number checks and index bounds.
    pub fn set_params(&self, index: usize, params: OscillatorParams) -> Result<(), String> {
        let mut validator = OscillatorBank::new(48_000.0, 0).map_err(|e| e.to_string())?;
        validator
            .set_params(index, params)
            .map_err(|e| e.to_string())?;
        self.controls
            .lock()
            .map_err(|_| "Synth control lock was poisoned".to_owned())?
            .oscillators[index] = params;
        Ok(())
    }

    pub fn set_volume(&self, value: f32) -> Result<(), String> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err("Volume must be finite and between 0 and 1".to_owned());
        }
        self.controls
            .lock()
            .map_err(|_| "Synth control lock was poisoned".to_owned())?
            .volume = value;
        Ok(())
    }

    /// Starts/retriggers one MIDI note. Repeated calls with the same note still
    /// retrigger. Phase resets wait for the short fade-out if already sounding.
    pub fn note_on(&self, note: u8) -> Result<(), String> {
        if note > 127 {
            return Err("MIDI note must be between 0 and 127".to_owned());
        }
        let mut controls = self
            .controls
            .lock()
            .map_err(|_| "Synth control lock was poisoned".to_owned())?;
        controls.note = Some(note);
        controls.generation = controls.generation.wrapping_add(1);
        Ok(())
    }

    /// Releases to exact silence over at most five milliseconds after the
    /// callback observes this command. No audio thread needs to be stopped.
    pub fn note_off(&self) -> Result<(), String> {
        self.controls
            .lock()
            .map_err(|_| "Synth control lock was poisoned".to_owned())?
            .note = None;
        Ok(())
    }
}

/// Owns the running stream: retain this value for as long as audio is needed.
/// Dropping it stops output. Device/format failures are returned by `start`;
/// later callback failures are available through `error` without locking.
pub struct AudioOutput {
    _stream: cpal::Stream,
    description: String,
    failed: Arc<AtomicBool>,
}

impl AudioOutput {
    pub fn start(synth: Synth) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "No default audio output device is available".to_owned())?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("Cannot read default audio configuration: {e}"))?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        if config.channels == 0 || config.sample_rate.0 == 0 {
            return Err("Audio device returned an invalid channel count or sample rate".to_owned());
        }
        let description = format!(
            "{} — {} Hz, {} channels, {format:?}",
            device
                .name()
                .map_err(|e| format!("Cannot read audio device name: {e}"))?,
            config.sample_rate.0,
            config.channels,
        );
        let failed = Arc::new(AtomicBool::new(false));
        let stream = match format {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config, synth, failed.clone()),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config, synth, failed.clone()),
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config, synth, failed.clone()),
            _ => {
                return Err(format!(
                    "Unsupported default audio sample format: {format:?}; expected f32, i16, or u16"
                ));
            }
        }?;
        stream
            .play()
            .map_err(|e| format!("Cannot start audio output: {e}"))?;
        Ok(Self {
            _stream: stream,
            description,
            failed,
        })
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    /// A sticky failure indicator. The backend error callback stores only an
    /// atomic flag; the UI/control thread allocates this message when requested.
    pub fn error(&self) -> Option<String> {
        self.failed.load(Ordering::Relaxed).then(|| {
            "Audio output stream failed. Check the output device and restart Plasma.".to_owned()
        })
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    synth: Synth,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |duration| duration.as_nanos() as u64);
    let mut renderer = Renderer {
        bank: OscillatorBank::new(f64::from(config.sample_rate.0), seed)
            .map_err(|e| e.to_string())?,
        controls: Controls::default(),
        active_generation: 0,
        gain: 0.0,
        gain_step: HEADROOM / (config.sample_rate.0 as f32 * RAMP_SECONDS).max(1.0),
    };
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _: &cpal::OutputCallbackInfo| {
                // Release the guard before DSP; contention defers controls one buffer.
                let snapshot = synth.controls.try_lock().ok().map(|guard| *guard);
                if let Some(snapshot) = snapshot {
                    renderer.update(snapshot);
                }
                let mut frames = output.chunks_exact_mut(channels);
                for frame in &mut frames {
                    let [left, right] = renderer.next_frame();
                    if channels == 1 {
                        frame[0] = T::from_sample((left + right) * 0.5);
                    } else {
                        frame[0] = T::from_sample(left);
                        frame[1] = T::from_sample(right);
                        // Only the front L/R pair is driven on surround devices.
                        for sample in &mut frame[2..] {
                            *sample = T::from_sample(0.0);
                        }
                    }
                }
                for sample in frames.into_remainder() {
                    *sample = T::from_sample(0.0);
                }
            },
            move |_| failed.store(true, Ordering::Relaxed),
            None,
        )
        .map_err(|e| format!("Cannot create audio output stream: {e}"))
}

struct Renderer {
    bank: OscillatorBank,
    controls: Controls,
    active_generation: u64,
    gain: f32,
    gain_step: f32,
}

impl Renderer {
    fn update(&mut self, controls: Controls) {
        for (index, params) in controls.oscillators.iter().enumerate() {
            if *params != self.controls.oscillators[index] {
                // Already validated by the control thread; no formatting here.
                let _ = self.bank.set_params(index, *params);
            }
        }
        self.controls = controls;
    }

    fn next_frame(&mut self) -> [f32; 2] {
        let retrigger = self.active_generation != self.controls.generation;
        if self.gain == 0.0 && retrigger {
            if let Some(note) = self.controls.note {
                let frequency = 440.0 * 2.0_f64.powf((f64::from(note) - 69.0) / 12.0);
                let _ = self.bank.note_on(frequency);
                self.active_generation = self.controls.generation;
            }
        }
        let target =
            if self.controls.note.is_some() && self.active_generation == self.controls.generation {
                self.controls.volume * HEADROOM
            } else {
                0.0
            };
        if self.gain < target {
            self.gain = (self.gain + self.gain_step).min(target);
        } else {
            self.gain = (self.gain - self.gain_step).max(target);
        }
        if self.gain == 0.0 {
            return [0.0; 2];
        }
        let frame = self.bank.next_frame();
        [
            (frame[0] * self.gain).clamp(-1.0, 1.0),
            (frame[1] * self.gain).clamp(-1.0, 1.0),
        ]
    }
}
