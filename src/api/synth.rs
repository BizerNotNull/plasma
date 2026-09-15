use crate::error::Error;
use crate::events::NoteQueue;
use crate::snapshot::{Controls, Meters, Published};
use plasma_kernel::{FilterMode, LfoWave, OscillatorParams, Telemetry, VoiceParams};
use std::sync::{Arc, Mutex};

/// Cloneable control handle. Telemetry is real DSP state, sampled per buffer;
/// individual meter values are atomic, but a whole meter frame is approximate.
#[derive(Clone, Default)]
pub struct Synth {
    pub(crate) controls: Arc<Mutex<Controls>>,
    pub(crate) published: Arc<Published>,
    pub(crate) meters: Arc<Meters>,
    pub(crate) notes: Arc<NoteQueue>,
}

impl Synth {
    pub fn new() -> Self {
        Self::default()
    }

    fn update(&self, f: impl FnOnce(&mut Controls) -> Result<(), Error>) -> Result<(), Error> {
        let mut guard = self.controls.lock().map_err(|_| Error::LockPoisoned)?;
        let mut next = *guard;
        f(&mut next)?;
        next.params.validate()?;
        *guard = next;
        self.published.store(next);
        Ok(())
    }

    pub fn voice_params(&self) -> Result<VoiceParams, Error> {
        Ok(self
            .controls
            .lock()
            .map_err(|_| Error::LockPoisoned)?
            .params)
    }

    pub fn params(&self, index: usize) -> Result<OscillatorParams, Error> {
        self.voice_params()?
            .oscillators
            .get(index)
            .copied()
            .ok_or(Error::InvalidOscillator)
    }

    pub fn set_params(&self, index: usize, params: OscillatorParams) -> Result<(), Error> {
        self.update(|c| {
            *c.params
                .oscillators
                .get_mut(index)
                .ok_or(Error::InvalidOscillator)? = params;
            Ok(())
        })
    }

    pub fn set_volume(&self, value: f32) -> Result<(), Error> {
        self.update(|c| {
            c.params.volume = value;
            Ok(())
        })
    }

    pub fn set_global(&self, index: usize, value: f32) -> Result<(), Error> {
        self.update(|c| {
            *c.params
                .globals
                .get_mut(index)
                .ok_or(Error::InvalidGlobal)? = value;
            Ok(())
        })
    }

    pub fn set_lfo_wave(&self, wave: LfoWave) -> Result<(), Error> {
        self.update(|c| {
            c.params.lfo_wave = wave;
            Ok(())
        })
    }

    pub fn set_lfo_retrigger(&self, retrigger: bool) -> Result<(), Error> {
        self.update(|c| {
            c.params.lfo_retrigger = retrigger;
            Ok(())
        })
    }

    pub fn set_filter_mode(&self, mode: FilterMode) -> Result<(), Error> {
        self.update(|c| {
            c.params.filter_mode = mode;
            Ok(())
        })
    }

    pub fn set_glide(&self, seconds: f32) -> Result<(), Error> {
        self.update(|c| {
            c.params.glide = seconds;
            Ok(())
        })
    }

    /// Overlapping notes reuse one voice. Releasing the sounding note retunes
    /// to the previous still-held key (last-note priority) instead of going idle.
    pub fn set_legato(&self, legato: bool) -> Result<(), Error> {
        self.update(|c| {
            c.params.legato = legato;
            Ok(())
        })
    }

    /// Slide from the last pitch even when envelopes retrigger. Off (default)
    /// is fingered: glide only on overlapping legato notes.
    pub fn set_always_glide(&self, always: bool) -> Result<(), Error> {
        self.update(|c| {
            c.params.always_glide = always;
            Ok(())
        })
    }

    /// Damper pedal. Note-off of unheld keys is deferred until sustain is released.
    /// Pedaled slots are stolen before physically held notes. All-notes-off still
    /// releases immediately.
    pub fn set_sustain(&self, sustain: bool) -> Result<(), Error> {
        self.update(|c| {
            c.params.sustain = sustain;
            Ok(())
        })
    }

    /// Channel pitch bend, -1..=1. Playback is `2^(bend * range / 12)` times the
    /// note/glide frequency. Does not retrigger or change key tracking.
    pub fn set_pitch_bend(&self, amount: f32) -> Result<(), Error> {
        self.update(|c| {
            c.params.pitch_bend = amount;
            Ok(())
        })
    }

    /// Pitch-bend range in semitones, 0..=24. Default 2.
    pub fn set_pitch_bend_range(&self, semitones: f32) -> Result<(), Error> {
        self.update(|c| {
            c.params.pitch_bend_range = semitones;
            Ok(())
        })
    }

    /// Channel modulation wheel, 0..=1. Unipolar matrix source 5. Live updates
    /// do not retrigger or change velocity/key tracking.
    pub fn set_mod_wheel(&self, amount: f32) -> Result<(), Error> {
        self.update(|c| {
            c.params.mod_wheel = amount;
            Ok(())
        })
    }

    /// Channel aftertouch (channel pressure), 0..=1. Unipolar matrix source 6.
    /// Live updates do not retrigger or change velocity/key tracking.
    pub fn set_aftertouch(&self, amount: f32) -> Result<(), Error> {
        self.update(|c| {
            c.params.aftertouch = amount;
            Ok(())
        })
    }

    /// White noise mixed into the filter, 0..=1. Independent of oscillator levels.
    pub fn set_noise(&self, amount: f32) -> Result<(), Error> {
        self.update(|c| {
            c.params.noise = amount;
            Ok(())
        })
    }

    /// Pre-filter tanh drive, 0..=1. Zero is linear (no saturator). Independent
    /// of oscillator levels and the noise mixer.
    pub fn set_drive(&self, amount: f32) -> Result<(), Error> {
        self.update(|c| {
            c.params.drive = amount;
            Ok(())
        })
    }

    /// Hard-sync this oscillator to oscillator 0. Index 0 is stored but ignored.
    pub fn set_sync(&self, index: usize, sync: bool) -> Result<(), Error> {
        self.update(|c| {
            *c.params
                .sync
                .get_mut(index)
                .ok_or(Error::InvalidOscillator)? = sync;
            Ok(())
        })
    }

    /// Linear through-zero FM from oscillator 0. Amount is 0..=1 (index 0..=8).
    /// Index 0 is one-sample self-FM. Independent of oscillator 0's audible level.
    pub fn set_fm(&self, index: usize, amount: f32) -> Result<(), Error> {
        self.update(|c| {
            *c.params
                .fm
                .get_mut(index)
                .ok_or(Error::InvalidOscillator)? = amount;
            Ok(())
        })
    }

    /// Ring modulation from oscillator 0. Amount is 0..=1.
    /// Index 0 is one-sample self-ring. Independent of oscillator 0's audible level.
    pub fn set_ring(&self, index: usize, amount: f32) -> Result<(), Error> {
        self.update(|c| {
            *c.params
                .ring
                .get_mut(index)
                .ok_or(Error::InvalidOscillator)? = amount;
            Ok(())
        })
    }

    /// Unison stereo spread around this oscillator's pan, 0..=1.
    /// Zero keeps every unison voice at that pan.
    pub fn set_spread(&self, index: usize, amount: f32) -> Result<(), Error> {
        self.update(|c| {
            *c.params
                .spread
                .get_mut(index)
                .ok_or(Error::InvalidOscillator)? = amount;
            Ok(())
        })
    }

    /// AMP ENV, LFO, MOD ENV, velocity, key tracking, mod wheel and aftertouch can address every target,
    /// including noise (40), spread (41..43), FM (44..45), ring (46..47), glide (48),
    /// oscillator 0 self-FM/ring (49..50) and oscillator 1/2 hard-sync (51..52, threshold 0.5).
    /// Source indices: 0 AMP ENV, 1 LFO, 2 MOD ENV, 3 velocity (0..1),
    /// 4 key tracking (MIDI 60 = 0, 60 semitones/unit, clamped to -1..1),
    /// 5 channel mod wheel (0..1), 6 channel aftertouch (0..1).
    pub fn set_route(&self, target: usize, source: usize, depth: f32) -> Result<(), Error> {
        self.update(|c| {
            *c.params
                .routes
                .get_mut(source)
                .and_then(|r| r.get_mut(target))
                .ok_or(Error::InvalidRoute)? = depth;
            Ok(())
        })
    }

    pub fn telemetry(&self) -> Telemetry {
        self.meters.load()
    }

    /// Queues a MIDI note in FIFO order; velocity zero queues note-off.
    /// At capacity, returns an error and requests release of all voices. Events
    /// queued before that request are discarded at the next buffer boundary.
    pub fn note_on(&self, note: u8, velocity: u8) -> Result<(), Error> {
        if note > 127 || velocity > 127 {
            return Err(Error::InvalidMidi);
        }
        self.notes.push(note, velocity)
    }

    /// Queues release of this note only. Other held notes continue sounding.
    pub fn note_off(&self, note: u8) -> Result<(), Error> {
        self.note_on(note, 0)
    }

    /// Requests release of every voice, even when the FIFO is full. Discards
    /// earlier queued events; later events remain ordered after this boundary.
    pub fn all_notes_off(&self) -> Result<(), Error> {
        self.notes.reset();
        Ok(())
    }
}
