//! One oscillator with up to four symmetrically detuned unison voices.
//! Oscillators 1 and 2 may hard-sync to oscillator 0's first unison wrap,
//! may be linearly frequency-modulated by oscillator 0's first-unison waveform,
//! and may ring-modulate against that same pre-gain sample. Oscillator 0 uses
//! that sample delayed by one frame as self-FM and self-ring. Unison voices may
//! be stereo-spread around the oscillator pan.

use crate::{Error, dsp};

pub const MAX_UNISON: usize = 4;

mod bank;
pub use bank::OscillatorBank;

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
    /// Equal-power pan, -1 (left)..=1 (right). Spread offsets each unison voice around this.
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
    gains: [[f64; 2]; MAX_UNISON],
    spread: f64,
    wrapped: bool,
    sync: bool,
    fm: f64,
    ring: f64,
    modulator: f64,
    modulator_valid: bool,
}

impl Oscillator {
    pub(crate) fn new(params: OscillatorParams) -> Self {
        Self {
            params,
            phases: [0.0; MAX_UNISON],
            steps: [0.0; MAX_UNISON],
            gains: [[0.0; 2]; MAX_UNISON],
            spread: 0.0,
            wrapped: false,
            sync: false,
            fm: 0.0,
            ring: 0.0,
            modulator: 0.0,
            modulator_valid: false,
        }
    }

    pub(crate) fn update(&mut self, frequency: f64, sample_rate: f64) {
        let p = self.params;
        let center = frequency * 2.0_f64.powf((p.pitch + p.fine / 100.0) / 12.0);
        let level = p.level / f64::from(p.unison);
        for i in 0..MAX_UNISON {
            let position = if p.unison == 1 {
                0.0
            } else {
                2.0 * i as f64 / (p.unison - 1) as f64 - 1.0
            };
            let increment = center * 2.0_f64.powf(position * p.detune / 1200.0) / sample_rate;
            // Unrepresentable frequencies are silent, never folded or clamped in pitch.
            self.steps[i] = if increment > 0.0 && increment < 0.5 {
                increment
            } else {
                0.0
            };
            let voice_pan = (p.pan + position * self.spread).clamp(-1.0, 1.0);
            let angle = (voice_pan + 1.0) * std::f64::consts::FRAC_PI_4;
            let (right, left) = angle.sin_cos();
            self.gains[i] = [left * level, right * level];
        }
    }

    pub(crate) fn trigger(&mut self, random: &mut dsp::Random) {
        self.modulator = 0.0;
        self.modulator_valid = false;
        for phase in &mut self.phases {
            *phase = (self.params.phase + self.params.phase_random * random.unit()).rem_euclid(1.0);
        }
    }

    pub(crate) fn wrapped(&self) -> bool {
        self.wrapped
    }

    pub(crate) fn hard_sync(&mut self) {
        self.phases = [0.0; MAX_UNISON];
    }

    pub(crate) fn next(&mut self, fm_mod: f64) -> [f64; 2] {
        let mut frame = [0.0; 2];
        self.wrapped = false;
        self.modulator = 0.0;
        let fm = if self.fm > 0.0 {
            8.0 * self.fm * fm_mod
        } else {
            0.0
        };
        for i in 0..usize::from(self.params.unison) {
            let step = self.steps[i];
            if step == 0.0 {
                continue;
            }
            let inc = if fm == 0.0 { step } else { step * (1.0 + fm) };
            if inc != 0.0 && inc.abs() < 0.5 {
                let sample = dsp::waveform(
                    self.params.waveform,
                    self.phases[i],
                    inc.abs(),
                    self.params.pulse_width,
                );
                if i == 0 {
                    self.modulator = sample;
                }
                frame[0] += sample * self.gains[i][0];
                frame[1] += sample * self.gains[i][1];
            }
            self.phases[i] += inc;
            if fm == 0.0 {
                if self.phases[i] >= 1.0 {
                    self.phases[i] -= 1.0;
                    if i == 0 {
                        self.wrapped = true;
                    }
                }
            } else if self.phases[i] >= 1.0 || self.phases[i] < 0.0 {
                self.phases[i] = self.phases[i].rem_euclid(1.0);
                if i == 0 {
                    self.wrapped = true;
                }
            }
        }
        frame
    }
}
