use crate::osc::Waveform;

// PolyBLEP corrects discontinuities over one sample on either side of an edge.
fn poly_blep(phase: f64, step: f64) -> f64 {
    if phase < step {
        let t = phase / step;
        t + t - t * t - 1.0
    } else if phase > 1.0 - step {
        let t = (phase - 1.0) / step;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

pub(crate) fn waveform(kind: Waveform, phase: f64, step: f64, width: f64) -> f64 {
    match kind {
        Waveform::Sine => (std::f64::consts::TAU * phase).sin(),
        // Continuous but not band-limited; corners can alias at high pitches.
        Waveform::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
        Waveform::Saw => 2.0 * phase - 1.0 - poly_blep(phase, step),
        Waveform::Pulse => {
            // Keep both edges resolvable at high frequencies.
            let width = width.clamp(step, 1.0 - step);
            let raw = if phase < width { 1.0 } else { -1.0 };
            raw + poly_blep(phase, step) - poly_blep((phase - width).rem_euclid(1.0), step)
        }
    }
}

pub(crate) struct Random(u64);

impl Random {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn unit(&mut self) -> f64 {
        // SplitMix64: explicit seed, no global state or audio-thread OS calls.
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^= z >> 31;
        (z >> 11) as f64 * (1.0 / ((1u64 << 53) as f64))
    }
}
