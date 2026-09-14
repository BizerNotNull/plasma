use plasma_kernel::{
    FilterMode, GLOBAL_COUNT, LfoWave, OSCILLATOR_COUNT, OscillatorParams, SOURCE_COUNT,
    TARGET_COUNT, Telemetry, VoiceParams, Waveform,
};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Clone, Copy, Default)]
pub(crate) struct Controls {
    pub(crate) params: VoiceParams,
}

const WORDS: usize =
    OSCILLATOR_COUNT * 10 + 7 + 3 * OSCILLATOR_COUNT + GLOBAL_COUNT + SOURCE_COUNT * TARGET_COUNT;

pub(crate) struct Published {
    version: AtomicU64,
    words: [AtomicU64; WORDS],
}

impl Default for Published {
    fn default() -> Self {
        let result = Self {
            version: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        };
        result.store(Controls::default());
        result
    }
}

impl Published {
    pub(crate) fn store(&self, c: Controls) {
        let mut words = [0; WORDS];
        let mut n = 0;
        for p in c.params.oscillators {
            for v in [
                p.waveform as u8 as f64,
                p.pitch,
                p.fine,
                p.phase,
                p.phase_random,
                p.pulse_width,
                p.unison as f64,
                p.detune,
                p.pan,
                p.level,
            ] {
                words[n] = v.to_bits();
                n += 1;
            }
        }
        words[n] = (c.params.volume as f64).to_bits();
        n += 1;
        for v in c.params.globals {
            words[n] = (v as f64).to_bits();
            n += 1;
        }
        words[n] = c.params.lfo_wave as u64;
        n += 1;
        words[n] = u64::from(c.params.lfo_retrigger);
        n += 1;
        words[n] = c.params.filter_mode as u64;
        n += 1;
        words[n] = (c.params.glide as f64).to_bits();
        n += 1;
        words[n] = u64::from(c.params.legato);
        n += 1;
        words[n] = (c.params.noise as f64).to_bits();
        n += 1;
        for s in c.params.sync {
            words[n] = u64::from(s);
            n += 1;
        }
        for v in c.params.fm {
            words[n] = (v as f64).to_bits();
            n += 1;
        }
        for v in c.params.ring {
            words[n] = (v as f64).to_bits();
            n += 1;
        }
        for v in c.params.routes.into_iter().flatten() {
            words[n] = (v as f64).to_bits();
            n += 1;
        }
        self.version.fetch_add(1, Ordering::SeqCst);
        for (dst, word) in self.words.iter().zip(words) {
            dst.store(word, Ordering::SeqCst);
        }
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn load(&self) -> Option<Controls> {
        let version = self.version.load(Ordering::SeqCst);
        if version & 1 != 0 {
            return None;
        }
        let words: [u64; WORDS] = std::array::from_fn(|i| self.words[i].load(Ordering::SeqCst));
        if self.version.load(Ordering::SeqCst) != version {
            return None;
        }
        let mut c = Controls::default();
        let mut n = 0;
        for p in &mut c.params.oscillators {
            let v: [f64; 10] = std::array::from_fn(|i| f64::from_bits(words[n + i]));
            n += 10;
            *p = OscillatorParams {
                waveform: match v[0] as u8 {
                    1 => Waveform::Triangle,
                    2 => Waveform::Saw,
                    3 => Waveform::Pulse,
                    _ => Waveform::Sine,
                },
                pitch: v[1],
                fine: v[2],
                phase: v[3],
                phase_random: v[4],
                pulse_width: v[5],
                unison: v[6] as u8,
                detune: v[7],
                pan: v[8],
                level: v[9],
            };
        }
        c.params.volume = f64::from_bits(words[n]) as f32;
        n += 1;
        for v in &mut c.params.globals {
            *v = f64::from_bits(words[n]) as f32;
            n += 1;
        }
        c.params.lfo_wave = match words[n] {
            1 => LfoWave::Triangle,
            2 => LfoWave::Saw,
            3 => LfoWave::Square,
            _ => LfoWave::Sine,
        };
        n += 1;
        c.params.lfo_retrigger = words[n] != 0;
        n += 1;
        c.params.filter_mode = match words[n] {
            1 => FilterMode::Bandpass,
            2 => FilterMode::Highpass,
            _ => FilterMode::Lowpass,
        };
        n += 1;
        c.params.glide = f64::from_bits(words[n]) as f32;
        n += 1;
        c.params.legato = words[n] != 0;
        n += 1;
        c.params.noise = f64::from_bits(words[n]) as f32;
        n += 1;
        for s in &mut c.params.sync {
            *s = words[n] != 0;
            n += 1;
        }
        for v in &mut c.params.fm {
            *v = f64::from_bits(words[n]) as f32;
            n += 1;
        }
        for v in &mut c.params.ring {
            *v = f64::from_bits(words[n]) as f32;
            n += 1;
        }
        for v in c.params.routes.iter_mut().flatten() {
            *v = f64::from_bits(words[n]) as f32;
            n += 1;
        }
        Some(c)
    }
}

pub(crate) struct Meters {
    values: [AtomicU32; TARGET_COUNT + SOURCE_COUNT],
}

impl Default for Meters {
    fn default() -> Self {
        let meters = Self {
            values: std::array::from_fn(|_| AtomicU32::new(0)),
        };
        meters.store(Telemetry::default());
        meters
    }
}

impl Meters {
    pub(crate) fn store(&self, t: Telemetry) {
        self.values[0].store(t.env.to_bits(), Ordering::Relaxed);
        self.values[1].store(t.lfo.to_bits(), Ordering::Relaxed);
        self.values[2].store(t.mod_env.to_bits(), Ordering::Relaxed);
        self.values[3].store(t.velocity.to_bits(), Ordering::Relaxed);
        self.values[4].store(t.key_track.to_bits(), Ordering::Relaxed);
        for (dst, v) in self.values[SOURCE_COUNT..].iter().zip(t.effective) {
            dst.store(v.to_bits(), Ordering::Relaxed);
        }
    }

    pub(crate) fn load(&self) -> Telemetry {
        Telemetry {
            env: f32::from_bits(self.values[0].load(Ordering::Relaxed)),
            lfo: f32::from_bits(self.values[1].load(Ordering::Relaxed)),
            mod_env: f32::from_bits(self.values[2].load(Ordering::Relaxed)),
            velocity: f32::from_bits(self.values[3].load(Ordering::Relaxed)),
            key_track: f32::from_bits(self.values[4].load(Ordering::Relaxed)),
            effective: std::array::from_fn(|i| {
                f32::from_bits(self.values[i + SOURCE_COUNT].load(Ordering::Relaxed))
            }),
        }
    }
}
