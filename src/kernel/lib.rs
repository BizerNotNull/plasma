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
//! other than 50%. Parameters are copied in at control rate, not shared atomically.

mod dsp;
pub mod osc;

pub use osc::{OscillatorParams, Waveform};

pub const OSCILLATOR_COUNT: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidSampleRate,
    InvalidFrequency,
    InvalidOscillatorIndex,
    InvalidParameter(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSampleRate => f.write_str("sample rate must be finite and positive"),
            Self::InvalidFrequency => f.write_str("frequency must be finite and nonnegative"),
            Self::InvalidOscillatorIndex => f.write_str("oscillator index must be in 0..3"),
            Self::InvalidParameter(name) => write!(f, "invalid oscillator parameter: {name}"),
        }
    }
}

impl std::error::Error for Error {}

pub struct OscillatorBank {
    oscillators: [osc::Oscillator; OSCILLATOR_COUNT],
    sample_rate: f64,
    frequency: f64,
    random: dsp::Random,
}

impl OscillatorBank {
    /// Initially silent. Oscillator 0 defaults to level 1, the others to level 0.
    /// The same seed and sequence of calls produce the same random phases.
    pub fn new(sample_rate: f64, seed: u64) -> Result<Self, Error> {
        check_sample_rate(sample_rate)?;
        Ok(Self {
            oscillators: std::array::from_fn(|i| {
                osc::Oscillator::new(OscillatorParams {
                    level: if i == 0 { 1.0 } else { 0.0 },
                    ..Default::default()
                })
            }),
            sample_rate,
            frequency: 0.0,
            random: dsp::Random::new(seed),
        })
    }

    pub fn params(&self, index: usize) -> Result<OscillatorParams, Error> {
        self.oscillators
            .get(index)
            .map(|osc| osc.params)
            .ok_or(Error::InvalidOscillatorIndex)
    }

    /// Invalid updates leave the bank unchanged. Phase changes wait for note_on.
    /// Changing unison does not retrigger: newly active voices retain their phase.
    pub fn set_params(&mut self, index: usize, params: OscillatorParams) -> Result<(), Error> {
        let osc = self
            .oscillators
            .get_mut(index)
            .ok_or(Error::InvalidOscillatorIndex)?;
        params.validate()?;
        osc.params = params;
        osc.update(self.frequency, self.sample_rate);
        Ok(())
    }

    /// Retunes without resetting phase. Zero silences the bank and freezes phase.
    /// Individual voices at or above Nyquist are also silent and frozen.
    pub fn set_frequency(&mut self, frequency: f64) -> Result<(), Error> {
        if !frequency.is_finite() || frequency < 0.0 {
            return Err(Error::InvalidFrequency);
        }
        self.frequency = frequency;
        self.update();
        Ok(())
    }

    /// Retunes and resets every voice to Phase + uniform(0, Phase Random).
    pub fn note_on(&mut self, frequency: f64) -> Result<(), Error> {
        self.set_frequency(frequency)?;
        for osc in &mut self.oscillators {
            osc.trigger(&mut self.random);
        }
        Ok(())
    }

    /// Preserves phase and pitch when the audio device's sample rate changes.
    pub fn set_sample_rate(&mut self, sample_rate: f64) -> Result<(), Error> {
        check_sample_rate(sample_rate)?;
        self.sample_rate = sample_rate;
        self.update();
        Ok(())
    }

    /// Produces one [left, right] frame. No heap allocation or locks.
    pub fn next_frame(&mut self) -> [f32; 2] {
        let mut frame = [0.0; 2];
        for osc in &mut self.oscillators {
            let sample = osc.next();
            frame[0] += sample[0];
            frame[1] += sample[1];
        }
        [frame[0] as f32, frame[1] as f32]
    }

    /// Overwrites caller-owned frames; does not add to existing buffer contents.
    pub fn render(&mut self, output: &mut [[f32; 2]]) {
        for frame in output {
            *frame = self.next_frame();
        }
    }

    fn update(&mut self) {
        for osc in &mut self.oscillators {
            osc.update(self.frequency, self.sample_rate);
        }
    }
}

fn check_sample_rate(sample_rate: f64) -> Result<(), Error> {
    if sample_rate.is_finite() && sample_rate > 0.0 {
        Ok(())
    } else {
        Err(Error::InvalidSampleRate)
    }
}
