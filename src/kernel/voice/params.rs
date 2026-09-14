use crate::{Error, OSCILLATOR_COUNT, OscillatorParams};

pub const TARGET_COUNT: usize = 51;
pub const GLOBAL_COUNT: usize = 12;
pub const SOURCE_COUNT: usize = 5;
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterMode {
    #[default]
    Lowpass,
    Bandpass,
    Highpass,
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
        27 | 30 | 33 | 35 | 38 | 40..=47 | 49 | 50 => (0.0, 1.0, false),
        28 | 29 | 31 | 36 | 37 | 39 => (0.001, 10.0, true),
        32 => (0.01, 30.0, true),
        34 => (20.0, 20000.0, true),
        48 => (0.0, 2.0, false),
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
    pub filter_mode: FilterMode,
    /// Portamento time in seconds, 0..=2. Zero is instantaneous. Target 48.
    pub glide: f32,
    /// Overlapping notes slide on one voice instead of stacking.
    pub legato: bool,
    /// Channel pitch bend, -1..=1. Scales playback by `2^(bend * range / 12)`.
    pub pitch_bend: f32,
    /// Pitch-bend range in semitones, 0..=24. Default 2.
    pub pitch_bend_range: f32,
    /// White noise mixed into the filter, 0..=1. Independent of oscillator levels.
    pub noise: f32,
    /// Hard-sync each oscillator to oscillator 0's first unison wrap. Index 0 is ignored.
    pub sync: [bool; OSCILLATOR_COUNT],
    /// Linear FM from oscillator 0, 0..=1 (index 0..=8). Index 0 is one-sample self-FM.
    pub fm: [f32; OSCILLATOR_COUNT],
    /// Ring modulation from oscillator 0, 0..=1. Index 0 is one-sample self-ring.
    pub ring: [f32; OSCILLATOR_COUNT],
    /// Unison stereo spread around each oscillator pan, 0..=1.
    pub spread: [f32; OSCILLATOR_COUNT],
    /// [source: AMP ENV=0 / LFO=1 / MOD ENV=2 / Velocity=3 / KeyTrack=4][destination].
    /// Zero removes a route. Key tracking is centered on MIDI 60, at 60 semitones
    /// per unit; depths use normalized target travel, not exact cutoff tracking.
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
            filter_mode: FilterMode::Lowpass,
            glide: 0.0,
            legato: false,
            pitch_bend: 0.0,
            pitch_bend_range: 2.0,
            noise: 0.0,
            sync: [false; OSCILLATOR_COUNT],
            fm: [0.0; OSCILLATOR_COUNT],
            ring: [0.0; OSCILLATOR_COUNT],
            spread: [0.0; OSCILLATOR_COUNT],
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
        normalize(48, self.glide)?;
        for amount in self.fm {
            if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
                return Err(Error::InvalidParameter("fm"));
            }
        }
        for amount in self.ring {
            if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
                return Err(Error::InvalidParameter("ring"));
            }
        }
        for amount in self.spread {
            if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
                return Err(Error::InvalidParameter("spread"));
            }
        }
        if !self.noise.is_finite() || !(0.0..=1.0).contains(&self.noise) {
            return Err(Error::InvalidParameter("noise"));
        }
        if !self.pitch_bend.is_finite() || !(-1.0..=1.0).contains(&self.pitch_bend) {
            return Err(Error::InvalidParameter("pitch bend"));
        }
        if !self.pitch_bend_range.is_finite() || !(0.0..=24.0).contains(&self.pitch_bend_range) {
            return Err(Error::InvalidParameter("pitch bend range"));
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
        values[40] = self.noise;
        for (i, v) in self.spread.iter().enumerate() {
            values[41 + i] = *v;
        }
        values[44] = self.fm[1];
        values[45] = self.fm[2];
        values[46] = self.ring[1];
        values[47] = self.ring[2];
        values[48] = normalize(48, self.glide).unwrap_or(0.0);
        values[49] = self.fm[0];
        values[50] = self.ring[0];
        values
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Telemetry {
    pub env: f32,
    pub lfo: f32,
    pub mod_env: f32,
    /// Note velocity / 127; independent of the AMP ENV.
    pub velocity: f32,
    /// MIDI note relative to 60, divided by 60 and clamped to [-1, 1].
    pub key_track: f32,
    pub effective: [f32; TARGET_COUNT],
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            env: 0.0,
            lfo: 0.0,
            mod_env: 0.0,
            velocity: 0.0,
            key_track: 0.0,
            effective: VoiceParams::default().normalized(),
        }
    }
}
