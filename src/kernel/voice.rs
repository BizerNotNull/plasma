//! Monophonic voice with a 1 kHz modulation control clock. Source-control routes
//! use the previous control tick's AMP ENV/LFO/MOD ENV outputs, preventing loops.
//! Base parameters are never overwritten by modulation. Oscillator phase/random
//! are sampled at the next trigger; unison modulation rounds to whole voices.

use crate::{Error, OSCILLATOR_COUNT, OscillatorBank, OscillatorParams};

pub const TARGET_COUNT: usize = 40;
pub const GLOBAL_COUNT: usize = 12;
pub const SOURCE_COUNT: usize = 3;
pub const GLOBAL_DEFAULTS: [f32; GLOBAL_COUNT] = [
    0.01, 0.2, 0.7, 0.4, 1.0, 0.0, 18000.0, 0.1, 0.01, 0.2, 0.7, 0.4,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LfoWave {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
}

/// Target ranges in native units; time, rate and cutoff use logarithmic travel.
pub fn target_range(target: usize) -> Result<(f32, f32, bool), Error> {
    let range = match target {
        0..=26 => match target % 9 {
            0 => (-48.0, 48.0, false),
            1 => (-100.0, 100.0, false),
            2 | 3 | 8 => (0.0, 1.0, false),
            4 => (0.01, 0.99, false),
            5 => (1.0, 4.0, false),
            6 => (0.0, 100.0, false),
            _ => (-1.0, 1.0, false),
        },
        27 | 30 | 33 | 35 | 38 => (0.0, 1.0, false),
        28 | 29 | 31 | 36 | 37 | 39 => (0.001, 10.0, true),
        32 => (0.01, 30.0, true),
        34 => (20.0, 20000.0, true),
        _ => return Err(Error::InvalidParameter("modulation target")),
    };
    Ok(range)
}
pub fn normalize(target: usize, value: f32) -> Result<f32, Error> {
    let (min, max, log) = target_range(target)?;
    if !value.is_finite() || !(min..=max).contains(&value) {
        return Err(Error::InvalidParameter("target value"));
    }
    Ok((if log {
        (value / min).ln() / (max / min).ln()
    } else {
        (value - min) / (max - min)
    })
    .clamp(0.0, 1.0))
}
pub fn denormalize(target: usize, value: f32) -> Result<f32, Error> {
    let (min, max, log) = target_range(target)?;
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(Error::InvalidParameter("normalized value"));
    }
    Ok((if log {
        min * (max / min).powf(value)
    } else {
        min + (max - min) * value
    })
    .clamp(min, max))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceParams {
    pub oscillators: [OscillatorParams; OSCILLATOR_COUNT],
    pub volume: f32,
    /// AMP ADSR, LFO Hz/phase, cutoff Hz/resonance, then independent MOD ADSR.
    pub globals: [f32; GLOBAL_COUNT],
    pub lfo_wave: LfoWave,
    pub lfo_retrigger: bool,
    /// [source: AMP ENV=0 / LFO=1 / MOD ENV=2][destination]. Zero removes a route.
    pub routes: [[f32; TARGET_COUNT]; SOURCE_COUNT],
}
impl Default for VoiceParams {
    fn default() -> Self {
        Self {
            oscillators: std::array::from_fn(|i| OscillatorParams {
                level: if i == 0 { 1.0 } else { 0.0 },
                ..Default::default()
            }),
            volume: 0.25,
            globals: GLOBAL_DEFAULTS,
            lfo_wave: LfoWave::Sine,
            lfo_retrigger: true,
            routes: [[0.0; TARGET_COUNT]; SOURCE_COUNT],
        }
    }
}
impl VoiceParams {
    pub fn validate(&self) -> Result<(), Error> {
        for p in &self.oscillators {
            p.validate()?;
        }
        normalize(27, self.volume)?;
        for (i, v) in self.globals.iter().enumerate() {
            normalize(28 + i, *v)?;
        }
        for depth in self.routes.iter().flatten() {
            if !depth.is_finite() || !(-1.0..=1.0).contains(depth) {
                return Err(Error::InvalidParameter("route depth"));
            }
        }
        Ok(())
    }
    pub fn normalized(&self) -> [f32; TARGET_COUNT] {
        let mut values = [0.0; TARGET_COUNT];
        for (i, p) in self.oscillators.iter().enumerate() {
            let native = [
                p.pitch,
                p.fine,
                p.phase,
                p.phase_random,
                p.pulse_width,
                p.unison as f64,
                p.detune,
                p.pan,
                p.level,
            ];
            for (f, v) in native.into_iter().enumerate() {
                values[i * 9 + f] = normalize(i * 9 + f, v as f32).unwrap_or(0.0);
            }
        }
        values[27] = self.volume;
        for (i, v) in self.globals.iter().enumerate() {
            values[28 + i] = normalize(28 + i, *v).unwrap_or(0.0);
        }
        values
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Telemetry {
    pub env: f32,
    pub lfo: f32,
    pub mod_env: f32,
    pub effective: [f32; TARGET_COUNT],
}
impl Default for Telemetry {
    fn default() -> Self {
        Self {
            env: 0.0,
            lfo: 0.0,
            mod_env: 0.0,
            effective: VoiceParams::default().normalized(),
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

struct Envelope {
    stage: Stage,
    level: f32,
    release_start: f32,
}
impl Default for Envelope {
    fn default() -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
            release_start: 0.0,
        }
    }
}
impl Envelope {
    fn note_on(&mut self) {
        self.stage = Stage::Attack;
    }

    fn note_off(&mut self) {
        if self.stage != Stage::Idle && self.stage != Stage::Release {
            self.release_start = self.level;
            self.stage = Stage::Release;
        }
    }

    fn next(&mut self, adsr: [f32; 4], sample_rate: f32, smooth: f32) -> f32 {
        let [attack, decay, sustain, release] = adsr;
        let env = &mut self.level;
        match self.stage {
            Stage::Idle => *env = 0.0,
            Stage::Attack => {
                *env = (*env + 1.0 / (attack * sample_rate)).min(1.0);
                if *env >= 1.0 {
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                *env = (*env - (1.0 - sustain) / (decay * sample_rate)).max(sustain);
                if *env <= sustain {
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {
                *env += (sustain - *env) * smooth;
            }
            Stage::Release => {
                *env = (*env - self.release_start / (release * sample_rate)).max(0.0);
                if *env <= 0.0 {
                    self.stage = Stage::Idle;
                }
            }
        }
        *env
    }
}
#[derive(Default)]
struct Lowpass {
    s1: f64,
    s2: f64,
}
impl Lowpass {
    // Topology-preserving state-variable filter: stable through cutoff sweeps.
    fn next(&mut self, input: f64, g: f64, k: f64) -> f64 {
        let a = 1.0 / (1.0 + g * (g + k));
        let v1 = a * (self.s1 + g * (input - self.s2));
        let v2 = self.s2 + g * v1;
        self.s1 = 2.0 * v1 - self.s1;
        self.s2 = 2.0 * v2 - self.s2;
        if self.s1.abs() < 1e-24 {
            self.s1 = 0.0;
        }
        if self.s2.abs() < 1e-24 {
            self.s2 = 0.0;
        }
        v2
    }
}
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
        }
        Ok(())
    }
    /// Retriggers both envelopes from their current levels (no discontinuity).
    /// Free LFO keeps its phase; retrigger LFO starts at the effective phase knob.
    pub fn note_on(&mut self, frequency: f64) -> Result<(), Error> {
        if !frequency.is_finite() || frequency < 0.0 {
            return Err(Error::InvalidFrequency);
        }
        self.control_tick();
        self.bank.note_on(frequency)?;
        self.amp_env.note_on();
        self.mod_env.note_on();
        if self.params.lfo_retrigger {
            self.lfo_phase = 0.0;
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
    /// AMP ENV alone determines silence; MOD ENV cannot prolong allocation.
    pub fn is_silent(&self) -> bool {
        self.amp_env.stage == Stage::Idle
    }
    fn control_tick(&mut self) {
        for i in 0..TARGET_COUNT {
            self.telemetry.effective[i] = (self.base[i]
                + self.params.routes[0][i] * self.telemetry.env
                + self.params.routes[1][i] * self.telemetry.lfo
                + self.params.routes[2][i] * self.telemetry.mod_env)
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
    /// Clears the previous note's envelopes and filter when a polyphonic slot is
    /// reassigned. The free-running LFO and smoothed controls remain continuous.
    pub(crate) fn reset_note(&mut self) {
        self.amp_env = Envelope::default();
        self.mod_env = Envelope::default();
        self.telemetry.env = 0.0;
        self.telemetry.mod_env = 0.0;
        self.filters = std::array::from_fn(|_| Lowpass::default());
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
            (self.filters[i].next(frame[i] as f64, self.g, self.k)
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
