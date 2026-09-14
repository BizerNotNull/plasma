use super::{Oscillator, OscillatorParams};
use crate::{Error, OSCILLATOR_COUNT, check_sample_rate, dsp};

pub struct OscillatorBank {
    oscillators: [Oscillator; OSCILLATOR_COUNT],
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
                Oscillator::new(OscillatorParams {
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
