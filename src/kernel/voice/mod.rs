//! Monophonic voice with a 1 kHz modulation control clock. Source-control routes
//! use the previous control tick's AMP ENV/LFO/MOD ENV outputs, preventing loops.
//! Velocity and key tracking are per-note constants, retained through release.
//! Base parameters are never overwritten by modulation. Oscillator phase/random
//! are sampled at the next trigger; unison modulation rounds to whole voices.

mod envelope;
mod filter;
mod params;

pub use params::{
    GLOBAL_COUNT, GLOBAL_DEFAULTS, FilterMode, LfoWave, SOURCE_COUNT, TARGET_COUNT, Telemetry,
    VoiceParams, denormalize, normalize, target_range,
};

use crate::{Error, OSCILLATOR_COUNT, OscillatorBank};
use envelope::Envelope;
use filter::Lowpass;

pub struct Voice {
    bank: OscillatorBank,
    params: VoiceParams,
    base: [f32; TARGET_COUNT],
    telemetry: Telemetry,
    sample_rate: f64,
    clock: usize,
    period: usize,
    amp_env: Envelope,
    mod_env: Envelope,
    lfo_phase: f64,
    effective_globals: [f32; GLOBAL_COUNT],
    filters: [Lowpass; 2],
    gain: f64,
    g: f64,
    k: f64,
    target_g: f64,
    target_k: f64,
    smooth: f64,
    current_hz: f64,
    target_hz: f64,
    glide_remaining: f64,
}

impl Voice {
    pub fn new(sample_rate: f64, seed: u64) -> Result<Self, Error> {
        let bank = OscillatorBank::new(sample_rate, seed)?;
        let params = VoiceParams::default();
        let mut voice = Self {
            bank,
            base: params.normalized(),
            params,
            telemetry: Telemetry::default(),
            sample_rate,
            clock: 0,
            period: (sample_rate / 1000.0).round().max(1.0) as usize,
            amp_env: Envelope::default(),
            mod_env: Envelope::default(),
            lfo_phase: 0.0,
            effective_globals: GLOBAL_DEFAULTS,
            filters: std::array::from_fn(|_| Lowpass::default()),
            gain: 0.0,
            g: 0.0,
            k: 1.0,
            target_g: 0.0,
            target_k: 1.0,
            smooth: 1.0 - (-1.0 / (sample_rate * 0.003)).exp(),
            current_hz: 0.0,
            target_hz: 0.0,
            glide_remaining: 0.0,
        };
        voice.control_tick();
        voice.g = voice.target_g;
        voice.k = voice.target_k;
        Ok(voice)
    }

    pub fn params(&self) -> &VoiceParams {
        &self.params
    }

    pub fn set_params(&mut self, params: VoiceParams) -> Result<(), Error> {
        params.validate()?;
        if params != self.params {
            self.base = params.normalized();
            self.params = params;
            self.clock = 0;
            for i in 0..OSCILLATOR_COUNT {
                self.bank.set_sync(i, params.sync[i])?;
            }
        }
        Ok(())
    }

    /// Retriggers both envelopes from their current levels (no discontinuity),
    /// unless legato and AMP ENV is already running. Free LFO keeps its phase;
    /// retrigger LFO starts at the effective phase knob. Velocity must be 0..=127
    /// and affects modulation only, not amplitude. Key tracking is inferred from
    /// frequency, centered on MIDI 60 with 60 semitones per unit and clamped to
    /// [-1, 1]; zero frequency maps to -1. Invalid frequency or velocity leaves
    /// all state unchanged. Legato overlapping notes retune without oscillator
    /// retrigger; glide interpolates in octaves over [`VoiceParams::glide`].
    pub fn note_on(&mut self, frequency: f64, velocity: u8) -> Result<(), Error> {
        if !frequency.is_finite() || frequency < 0.0 {
            return Err(Error::InvalidFrequency);
        }
        if velocity > 127 {
            return Err(Error::InvalidParameter("MIDI velocity"));
        }
        self.telemetry.velocity = velocity as f32 / 127.0;
        self.telemetry.key_track = if frequency == 0.0 {
            -1.0
        } else {
            ((69.0 + 12.0 * (frequency.log2() - 440.0_f64.log2()) - 60.0) / 60.0).clamp(-1.0, 1.0)
                as f32
        };
        self.control_tick();
        let slide = self.params.legato && !self.amp_env.is_idle();
        self.target_hz = frequency;
        if slide {
            if self.params.glide <= 0.0 || self.current_hz <= 0.0 || frequency <= 0.0 {
                self.current_hz = frequency;
                self.glide_remaining = 0.0;
                self.bank.set_frequency(frequency)?;
            } else {
                self.glide_remaining = f64::from(self.params.glide);
            }
        } else {
            self.current_hz = frequency;
            self.glide_remaining = 0.0;
            self.bank.note_on(frequency)?;
            self.amp_env.note_on();
            self.mod_env.note_on();
            if self.params.lfo_retrigger {
                self.lfo_phase = 0.0;
            }
        }
        Ok(())
    }

    pub fn note_off(&mut self) {
        self.amp_env.note_off();
        self.mod_env.note_off();
    }

    pub fn telemetry(&self) -> Telemetry {
        self.telemetry
    }

    /// Instantaneous oscillator base frequency, including an in-progress glide.
    pub fn frequency(&self) -> f64 {
        self.current_hz
    }

    /// AMP ENV alone determines silence; MOD ENV cannot prolong allocation.
    pub fn is_silent(&self) -> bool {
        self.amp_env.is_idle()
    }

    fn control_tick(&mut self) {
        self.advance_glide();
        for i in 0..TARGET_COUNT {
            self.telemetry.effective[i] = (self.base[i]
                + self.params.routes[0][i] * self.telemetry.env
                + self.params.routes[1][i] * self.telemetry.lfo
                + self.params.routes[2][i] * self.telemetry.mod_env
                + self.params.routes[3][i] * self.telemetry.velocity
                + self.params.routes[4][i] * self.telemetry.key_track)
                .clamp(0.0, 1.0);
        }
        for i in 0..OSCILLATOR_COUNT {
            let mut p = self.params.oscillators[i];
            let v: [f64; 9] = std::array::from_fn(|f| {
                denormalize(i * 9 + f, self.telemetry.effective[i * 9 + f]).unwrap_or(0.0) as f64
            });
            p.pitch = v[0];
            p.fine = v[1];
            p.phase = v[2];
            p.phase_random = v[3];
            p.pulse_width = v[4].clamp(0.01, 0.99);
            p.unison = v[5].round() as u8;
            p.detune = v[6];
            p.pan = v[7];
            p.level = v[8];
            self.telemetry.effective[i * 9 + 5] = (p.unison as f32 - 1.0) / 3.0;
            if self.bank.params(i).ok() != Some(p) {
                let _ = self.bank.set_params(i, p);
            }
        }
        for i in 0..GLOBAL_COUNT {
            self.effective_globals[i] =
                denormalize(28 + i, self.telemetry.effective[28 + i]).unwrap_or(GLOBAL_DEFAULTS[i]);
        }
        let cutoff = (self.effective_globals[6] as f64).min(self.sample_rate * 0.45);
        self.target_g = (std::f64::consts::PI * cutoff / self.sample_rate).tan();
        self.target_k = 2.0 - 1.9 * self.effective_globals[7] as f64;
    }

    fn advance_glide(&mut self) {
        if self.params.glide <= 0.0 || self.glide_remaining <= 0.0 {
            if self.current_hz != self.target_hz {
                self.current_hz = self.target_hz;
                let _ = self.bank.set_frequency(self.current_hz);
            }
            self.glide_remaining = 0.0;
            return;
        }
        let dt = self.period as f64 / self.sample_rate;
        let step = dt.min(self.glide_remaining);
        let frac = step / self.glide_remaining;
        self.glide_remaining -= step;
        if self.current_hz > 0.0 && self.target_hz > 0.0 {
            let from = self.current_hz.log2();
            let to = self.target_hz.log2();
            self.current_hz = 2.0_f64.powf(from + (to - from) * frac);
        } else {
            self.current_hz = self.target_hz;
            self.glide_remaining = 0.0;
        }
        if self.glide_remaining <= 0.0 {
            self.current_hz = self.target_hz;
            self.glide_remaining = 0.0;
        }
        let _ = self.bank.set_frequency(self.current_hz);
    }

    /// Clears the previous note's envelopes, note sources and filter when a
    /// polyphonic slot is reassigned. Free LFO and smoothed controls stay continuous.
    pub(crate) fn reset_note(&mut self) {
        self.amp_env = Envelope::default();
        self.mod_env = Envelope::default();
        self.telemetry.env = 0.0;
        self.telemetry.mod_env = 0.0;
        self.telemetry.velocity = 0.0;
        self.telemetry.key_track = 0.0;
        self.filters = std::array::from_fn(|_| Lowpass::default());
        self.current_hz = 0.0;
        self.target_hz = 0.0;
        self.glide_remaining = 0.0;
    }

    /// One stereo frame, with no allocation, synchronization or shared state.
    pub fn next_frame(&mut self) -> [f32; 2] {
        self.next_frame_unclipped()
            .map(|sample| sample.clamp(-1.0, 1.0))
    }

    /// Polyphonic mixing applies its limiter only after summing all voices.
    pub(crate) fn next_frame_unclipped(&mut self) -> [f32; 2] {
        if self.clock == 0 {
            self.control_tick();
            self.clock = self.period;
        }
        self.clock -= 1;
        let [
            attack,
            decay,
            sustain,
            release,
            rate,
            phase,
            _,
            _,
            mod_attack,
            mod_decay,
            mod_sustain,
            mod_release,
        ] = self.effective_globals;
        self.telemetry.env = self.amp_env.next(
            [attack, decay, sustain, release],
            self.sample_rate as f32,
            self.smooth as f32,
        );
        self.telemetry.mod_env = self.mod_env.next(
            [mod_attack, mod_decay, mod_sustain, mod_release],
            self.sample_rate as f32,
            self.smooth as f32,
        );
        let p = (self.lfo_phase + phase as f64).fract();
        self.telemetry.lfo = match self.params.lfo_wave {
            LfoWave::Sine => (std::f64::consts::TAU * p).sin() as f32,
            LfoWave::Triangle => (1.0 - 4.0 * (p - 0.5).abs()) as f32,
            LfoWave::Saw => (2.0 * p - 1.0) as f32,
            LfoWave::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        };
        self.lfo_phase = (self.lfo_phase + rate as f64 / self.sample_rate).fract();
        self.g += (self.target_g - self.g) * self.smooth;
        self.k += (self.target_k - self.k) * self.smooth;
        self.gain += (self.telemetry.effective[27] as f64 * 0.8 / 3.0 - self.gain) * self.smooth;
        if self.is_silent() {
            self.filters[0] = Lowpass::default();
            self.filters[1] = Lowpass::default();
            return [0.0; 2];
        }
        let frame = self.bank.next_frame();
        std::array::from_fn(|i| {
            (self.filters[i].next(frame[i] as f64, self.g, self.k, self.params.filter_mode)
                * self.gain
                * self.telemetry.env as f64) as f32
        })
    }

    pub fn render(&mut self, output: &mut [[f32; 2]]) {
        for frame in output {
            *frame = self.next_frame();
        }
    }
}
