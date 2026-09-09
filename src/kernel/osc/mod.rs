//! One oscillator with up to four symmetrically detuned unison voices.

use crate::{Error, dsp};

pub const MAX_UNISON: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Waveform {
    #[default]
    Sine,
    Triangle,
    Saw,
    Pulse,
}

/// Control-rate parameters. Changes are immediate (no smoothing).
/// Phase controls take effect at the next `OscillatorBank::note_on`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OscillatorParams {
    pub waveform: Waveform,
    /// Semitones relative to the bank's base frequency, -48..=48.
    pub pitch: f64,
    /// Additional cents, -100..=100.
    pub fine: f64,
    /// Initial phase in cycles, 0..=1 (1 wraps to 0).
    pub phase: f64,
    /// Random forward phase offset in cycles, 0..=1, per voice and trigger.
    pub phase_random: f64,
    /// Pulse duty cycle, 0.01..=0.99. Only affects Pulse.
    pub pulse_width: f64,
    /// Number of active voices, 1..=4. Voices are averaged, not summed.
    pub unison: u8,
    /// Maximum offset from center in cents, 0..=100. One voice stays centered.
    pub detune: f64,
    /// Equal-power pan, -1 (left)..=1 (right); applies to all unison voices.
    pub pan: f64,
    /// Linear amplitude, 0..=1.
    pub level: f64,
}

impl Default for OscillatorParams {
    fn default() -> Self {
        Self {
            waveform: Waveform::Sine,
            pitch: 0.0,
            fine: 0.0,
            phase: 0.0,
            phase_random: 0.0,
            pulse_width: 0.5,
            unison: 1,
            detune: 0.0,
            pan: 0.0,
            level: 1.0,
        }
    }
}

impl OscillatorParams {
    pub(crate) fn validate(&self) -> Result<(), Error> {
        for (name, value, min, max) in [
            ("pitch", self.pitch, -48.0, 48.0),
            ("fine", self.fine, -100.0, 100.0),
            ("phase", self.phase, 0.0, 1.0),
            ("phase_random", self.phase_random, 0.0, 1.0),
            ("pulse_width", self.pulse_width, 0.01, 0.99),
            ("detune", self.detune, 0.0, 100.0),
            ("pan", self.pan, -1.0, 1.0),
            ("level", self.level, 0.0, 1.0),
        ] {
            if !value.is_finite() || !(min..=max).contains(&value) {
                return Err(Error::InvalidParameter(name));
            }
        }
        if !(1..=MAX_UNISON as u8).contains(&self.unison) {
            return Err(Error::InvalidParameter("unison"));
        }
        Ok(())
    }
}

pub(crate) struct Oscillator {
    pub(crate) params: OscillatorParams,
    phases: [f64; MAX_UNISON],
    steps: [f64; MAX_UNISON],
    gains: [f64; 2],
}

impl Oscillator {
    pub(crate) fn new(params: OscillatorParams) -> Self {
        Self {
            params,
            phases: [0.0; MAX_UNISON],
            steps: [0.0; MAX_UNISON],
            gains: [0.0; 2],
        }
    }

    pub(crate) fn update(&mut self, frequency: f64, sample_rate: f64) {
        let p = self.params;
        let center = frequency * 2.0_f64.powf((p.pitch + p.fine / 100.0) / 12.0);
        for (i, step) in self.steps.iter_mut().enumerate() {
            let position = if p.unison == 1 {
                0.0
            } else {
                2.0 * i as f64 / (p.unison - 1) as f64 - 1.0
            };
            let increment = center * 2.0_f64.powf(position * p.detune / 1200.0) / sample_rate;
            // Unrepresentable frequencies are silent, never folded or clamped in pitch.
            *step = if increment > 0.0 && increment < 0.5 {
                increment
            } else {
                0.0
            };
        }
        let angle = (p.pan + 1.0) * std::f64::consts::FRAC_PI_4;
        let (right, left) = angle.sin_cos();
        let level = p.level / f64::from(p.unison);
        self.gains = [left * level, right * level];
    }

    pub(crate) fn trigger(&mut self, random: &mut dsp::Random) {
        for phase in &mut self.phases {
            *phase = (self.params.phase + self.params.phase_random * random.unit()).rem_euclid(1.0);
        }
    }

    pub(crate) fn next(&mut self) -> [f64; 2] {
        let mut mono = 0.0;
        for i in 0..usize::from(self.params.unison) {
            let step = self.steps[i];
            if step == 0.0 {
                continue;
            }
            mono += dsp::waveform(
                self.params.waveform,
                self.phases[i],
                step,
                self.params.pulse_width,
            );
            self.phases[i] += step;
            if self.phases[i] >= 1.0 {
                self.phases[i] -= 1.0;
            }
        }
        [mono * self.gains[0], mono * self.gains[1]]
    }
}
