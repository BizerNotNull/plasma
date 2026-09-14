//! Eight-voice synthesis and real system audio. Parameters are atomic snapshots;
//! notes use a bounded FIFO shared by cloneable writers. The audio consumer never
//! locks, waits or allocates. Only one live renderer/output may consume a Synth.
mod audio;
mod error;
mod events;
mod snapshot;
mod synth;
#[cfg(test)]
mod tests;

pub use audio::{AudioOutput, AudioRenderer};
pub use error::Error;
pub use plasma_kernel::{
    GLOBAL_COUNT, GLOBAL_DEFAULTS, FilterMode, LfoWave, OscillatorParams, POLYPHONY, PolySynth,
    SOURCE_COUNT, TARGET_COUNT, Telemetry, Voice, VoiceParams, Waveform, denormalize, normalize,
    target_range,
};
pub use synth::Synth;
