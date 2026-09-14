//! Allocation-free, dependency-free, three-oscillator stereo source.
//!
//! ```
//! use plasma_kernel::{OscillatorBank, OscillatorParams, Waveform};
//! let mut bank = OscillatorBank::new(48_000.0, 42)?;
//! bank.set_params(0, OscillatorParams {
//!     waveform: Waveform::Saw, unison: 4, detune: 12.0, level: 0.2,
//!     ..Default::default()
//! })?;
//! bank.note_on(440.0)?;
//! let mut frames = [[0.0; 2]; 128];
//! bank.render(&mut frames);
//! # Ok::<(), plasma_kernel::Error>(())
//! ```
//!
//! Each bank represents one note, not a polyphonic synth. No envelope, filter,
//! limiter or parameter smoothing is applied. The three stereo outputs are
//! summed without clipping; callers must leave headroom. Saw/Pulse use PolyBLEP;
//! Triangle is a basic, non-band-limited waveform. Pulse can contain DC at duties
//! other than 50%. Oscillators 1 and 2 may hard-sync to oscillator 0's first
//! unison wrap, resetting every unison phase to 0, and may be linearly
//! frequency-modulated by oscillator 0's first-unison waveform (amount 0..=1 maps
//! to index 0..=8, independent of OSC 1 level). Parameters are copied in at
//! control rate, not shared atomically.
//!
//! [`Voice`] wraps the unchanged oscillator bank with independent AMP and MOD
//! ADSRs, a free or retriggered LFO, a stereo state-variable filter
//! (lowpass/bandpass/highpass) and a modulation matrix.
//! Targets 0..27 are the nine oscillator knobs per oscillator (pitch, fine,
//! phase, random phase, pulse width, unison, detune, pan, level); 27 is master
//! gain and 28..40 are the twelve [`VoiceParams::globals`] controls, with MOD ADSR
//! appended at 36..40. AMP ENV, LFO, MOD ENV, velocity and key tracking may route
//! to every target with signed
//! normalized depths. Only AMP ENV controls final amplitude and voice lifetime.
//! Velocity is a per-note unipolar source; key tracking is centered on MIDI 60
//! with 60 semitones per unit, clamped to [-1, 1]. Route depths represent
//! normalized target travel, not a percentage of exact cutoff tracking.
//! Filter coefficients and master gain are smoothed over 3 ms; modulation runs at approximately
//! 1 kHz, without recomputing unchanged oscillator coefficients.
//!
//! [`PolySynth`] supplies eight independent voices, MIDI note/velocity events,
//! selective release, deterministic stealing and a bounded stereo mix.

mod dsp;
mod error;
pub mod osc;
mod poly;
pub use error::Error;
pub use osc::{OscillatorBank, OscillatorParams, Waveform};
pub use poly::{POLYPHONY, PolySynth};
pub mod voice;
pub use voice::{
    GLOBAL_COUNT, GLOBAL_DEFAULTS, FilterMode, LfoWave, SOURCE_COUNT, TARGET_COUNT, Telemetry,
    Voice, VoiceParams, denormalize, normalize, target_range,
};

pub const OSCILLATOR_COUNT: usize = 3;

pub(crate) fn check_sample_rate(sample_rate: f64) -> Result<(), Error> {
    if sample_rate.is_finite() && sample_rate > 0.0 {
        Ok(())
    } else {
        Err(Error::InvalidSampleRate)
    }
}
