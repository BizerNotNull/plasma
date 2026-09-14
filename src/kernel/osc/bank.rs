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

    /// Index 0 is stored but ignored. Invalid indices leave the bank unchanged.
    pub fn set_sync(&mut self, index: usize, sync: bool) -> Result<(), Error> {
        let osc = self
            .oscillators
            .get_mut(index)
            .ok_or(Error::InvalidOscillatorIndex)?;
        osc.sync = sync;
        Ok(())
    }

    pub fn sync(&self, index: usize) -> Result<bool, Error> {
        self.oscillators
            .get(index)
            .map(|osc| osc.sync)
            .ok_or(Error::InvalidOscillatorIndex)
    }

    /// Linear through-zero FM from oscillator 0's first unison waveform.
    /// Amount is 0..=1 (internal index 0..=8). Index 0 is stored but ignored.
    /// Invalid indices or amounts leave the bank unchanged.
    pub fn set_fm(&mut self, index: usize, amount: f64) -> Result<(), Error> {
        let osc = self
            .oscillators
            .get_mut(index)
            .ok_or(Error::InvalidOscillatorIndex)?;
        if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
            return Err(Error::InvalidParameter("fm"));
        }
        osc.fm = amount;
        Ok(())
    }

    pub fn fm(&self, index: usize) -> Result<f64, Error> {
        self.oscillators
            .get(index)
            .map(|osc| osc.fm)
            .ok_or(Error::InvalidOscillatorIndex)
    }

    /// Ring modulation against oscillator 0's first unison waveform.
    /// Amount is 0..=1: `out = carrier * (1 - amount + amount * modulator)`.
    /// Index 0 is stored but ignored. Invalid indices or amounts leave the bank unchanged.
    /// Independent of oscillator 0's audible level.
    pub fn set_ring(&mut self, index: usize, amount: f64) -> Result<(), Error> {
        let osc = self
            .oscillators
            .get_mut(index)
            .ok_or(Error::InvalidOscillatorIndex)?;
        if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
            return Err(Error::InvalidParameter("ring"));
        }
        osc.ring = amount;
        Ok(())
    }

    pub fn ring(&self, index: usize) -> Result<f64, Error> {
        self.oscillators
            .get(index)
            .map(|osc| osc.ring)
            .ok_or(Error::InvalidOscillatorIndex)
    }

    /// Stereo spread of unison voices around the oscillator pan, 0..=1.
    /// Zero keeps every unison voice at that pan. Invalid indices or amounts
    /// leave the bank unchanged.
    pub fn set_spread(&mut self, index: usize, amount: f64) -> Result<(), Error> {
        let osc = self
            .oscillators
            .get_mut(index)
            .ok_or(Error::InvalidOscillatorIndex)?;
        if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
            return Err(Error::InvalidParameter("spread"));
        }
        osc.spread = amount;
        osc.update(self.frequency, self.sample_rate);
        Ok(())
    }

    pub fn spread(&self, index: usize) -> Result<f64, Error> {
        self.oscillators
            .get(index)
            .map(|osc| osc.spread)
            .ok_or(Error::InvalidOscillatorIndex)
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
    /// Oscillators 1 and 2 with `sync` reset when oscillator 0's first voice wraps.
    /// Their phase increment is `step * (1 + 8 * fm * modulator)` using oscillator 0's
    /// pre-gain first-unison sample, so FM works when oscillator 0's level is 0.
    /// Ring uses that same sample: `out * (1 - ring + ring * modulator)`.
    pub fn next_frame(&mut self) -> [f32; 2] {
        let mut frame = [0.0; 2];
        let mut master_wrapped = false;
        let mut master_mod = 0.0;
        for (i, osc) in self.oscillators.iter_mut().enumerate() {
            if i > 0 && osc.sync && master_wrapped {
                osc.hard_sync();
            }
            let mut sample = osc.next(if i == 0 { 0.0 } else { master_mod });
            if i == 0 {
                master_wrapped = osc.wrapped();
                master_mod = osc.modulator;
            } else if osc.ring > 0.0 {
                let scale = 1.0 - osc.ring + osc.ring * master_mod;
                sample[0] *= scale;
                sample[1] *= scale;
            }
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
