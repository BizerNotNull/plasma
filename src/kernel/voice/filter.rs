use super::params::FilterMode;

#[derive(Default)]
pub(super) struct Lowpass {
    s1: f64,
    s2: f64,
}

impl Lowpass {
    // Topology-preserving state-variable filter: stable through cutoff sweeps.
    pub(super) fn next(&mut self, input: f64, g: f64, k: f64, mode: FilterMode) -> f64 {
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
        match mode {
            FilterMode::Lowpass => v2,
            FilterMode::Bandpass => v1,
            FilterMode::Highpass => input - k * v1 - v2,
        }
    }
}
