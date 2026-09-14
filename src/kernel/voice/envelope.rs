#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

pub(super) struct Envelope {
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
    pub(super) fn note_on(&mut self) {
        self.stage = Stage::Attack;
    }

    pub(super) fn note_off(&mut self) {
        if self.stage != Stage::Idle && self.stage != Stage::Release {
            self.release_start = self.level;
            self.stage = Stage::Release;
        }
    }

    pub(super) fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    pub(super) fn next(&mut self, adsr: [f32; 4], sample_rate: f32, smooth: f32) -> f32 {
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
