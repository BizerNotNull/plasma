//! Fixed-capacity MIDI voice allocation and post-voice mixing.

use crate::{Error, Telemetry, Voice, VoiceParams, check_sample_rate};

pub const POLYPHONY: usize = 8;

struct Slot {
    voice: Voice,
    note: Option<u8>,
    held: bool,
    pedal: bool,
    // A permutation of 0..POLYPHONY; zero is most recently triggered.
    rank: usize,
    velocity: f32,
    last: [f32; 2],
    transition_from: [f32; 2],
    transition_left: usize,
}

/// Eight independently seeded oscillator/AMP ENV/MOD ENV/LFO/filter voices.
///
/// Repeated held notes retrigger their existing slot. Otherwise allocation uses
/// an idle slot, then the oldest released slot, then the oldest held slot.
/// Release tails count as active until AMP ENV and the transition finish;
/// a long MOD ENV release never retains an otherwise silent slot.
/// Age is a bounded rank permutation, so arbitrarily long event streams cannot
/// overflow a timestamp or change the stealing order.
///
/// Each trigger crossfades from the slot's last emitted sample to its new live
/// signal over 3 ms (at least two frames). This is a bounded sample-hold fade,
/// not a second oscillator voice: even repeated steals restart from the actual
/// last output, with no accumulated correction or extra rendering work. The
/// first frame after a trigger exactly preserves that slot's preceding output.
/// Velocity scales amplitude linearly by velocity / 127, before this fade, and
/// independently supplies each voice's Velocity modulation source.
/// Channel pitch bend is a voice parameter: every slot scales its glide/note
/// frequency by `2^(bend * range / 12)` without retriggering or changing key tracking.
/// Channel sustain (damper) defers note-off while the pedal is down. Releasing
/// sustain note-offs every unheld slot; all-notes-off still releases immediately.
/// Channel mod wheel is a unipolar matrix source shared by every slot; moving it
/// does not retrigger, steal, or change velocity/key tracking.
///
/// Legato reuses the most recently triggered held slot. Further keys are stored
/// in a fixed last-note stack: releasing the sounding note retunes to the
/// previous still-held key without releasing the envelope, and releasing a
/// non-sounding key only forgets it. Enabling legato captures currently held
/// keys in trigger order and releases extra voices so one remains, including
/// pedaled extras. A physically held slot is kept over a more recent pedaled
/// one. Polyphony is unchanged when legato is off.
///
/// Mixing uses fixed 1/8 headroom, independent of the active count, then clamps
/// only the final stereo sum to [-1, 1]. Construction, events and rendering use
/// fixed storage and never allocate, lock or wait.
pub struct PolySynth {
    slots: [Slot; POLYPHONY],
    transition_frames: usize,
    idle_telemetry: Telemetry,
    params: VoiceParams,
    held_notes: [u8; 128],
    held_velocities: [u8; 128],
    held_len: usize,
}

impl PolySynth {
    pub fn new(sample_rate: f64, seed: u64) -> Result<Self, Error> {
        check_sample_rate(sample_rate)?;
        Ok(Self {
            slots: std::array::from_fn(|i| Slot {
                // Sample rate has already been validated above.
                voice: Voice::new(
                    sample_rate,
                    seed.wrapping_add((i as u64).wrapping_mul(0x9e3779b97f4a7c15)),
                )
                .expect("validated sample rate"),
                note: None,
                held: false,
                pedal: false,
                rank: i,
                velocity: 0.0,
                last: [0.0; 2],
                transition_from: [0.0; 2],
                transition_left: 0,
            }),
            transition_frames: (sample_rate * 0.003).round().max(2.0) as usize,
            idle_telemetry: Telemetry::default(),
            params: VoiceParams::default(),
            held_notes: [0; 128],
            held_velocities: [0; 128],
            held_len: 0,
        })
    }

    /// Updates every slot, including idle ones; invalid snapshots change nothing.
    pub fn set_params(&mut self, params: VoiceParams) -> Result<(), Error> {
        if params == self.params {
            return Ok(());
        }
        params.validate()?;
        for slot in &mut self.slots {
            slot.voice.set_params(params)?;
        }
        let mut effective = params.normalized();
        for (value, depth) in effective.iter_mut().zip(params.routes[5]) {
            *value = (*value + depth * params.mod_wheel).clamp(0.0, 1.0);
        }
        self.idle_telemetry.effective = effective;
        self.idle_telemetry.mod_wheel = params.mod_wheel;
        let was_legato = self.params.legato;
        let was_sustain = self.params.sustain;
        self.params = params;
        if !self.params.legato {
            self.clear_held();
        } else if !was_legato {
            self.capture_held_keys();
        }
        if was_sustain && !self.params.sustain {
            self.release_unheld();
        }
        Ok(())
    }

    /// MIDI note and velocity must be in 0..=127. Velocity zero is note-off.
    /// Releasing a stolen note never releases its replacement.
    pub fn note_on(&mut self, note: u8, velocity: u8) -> Result<(), Error> {
        Self::validate_note(note)?;
        if velocity > 127 {
            return Err(Error::InvalidParameter("MIDI velocity"));
        }
        if velocity == 0 {
            return self.note_off(note);
        }
        if self.params.legato {
            self.push_held(note, velocity);
        }
        let repeated = self.slots.iter().position(|slot| {
            slot.note == Some(note) && (slot.held || slot.pedal)
        });
        let legato_held = if self.params.legato && repeated.is_none() {
            self.legato_slot()
        } else {
            None
        };
        let index = if let Some(index) = repeated {
            index
        } else if let Some(index) = legato_held {
            index
        } else {
            self.slots
                .iter()
                .position(|slot| slot.note.is_none())
                .or_else(|| {
                    self.slots
                        .iter()
                        .enumerate()
                        .filter(|(_, slot)| !slot.held)
                        .max_by_key(|(_, slot)| slot.rank)
                        .map(|(i, _)| i)
                })
                .unwrap_or_else(|| {
                    self.slots
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, slot)| slot.rank)
                        .map(|(i, _)| i)
                        .expect("eight slots")
                })
        };
        self.promote(index);
        let slot = &mut self.slots[index];
        if repeated.is_none() && legato_held.is_none() {
            slot.voice.reset_note();
        }
        slot.voice.note_on(midi_hz(note), velocity)?;
        slot.note = Some(note);
        slot.held = true;
        slot.pedal = false;
        slot.velocity = velocity as f32 / 127.0;
        slot.transition_from = slot.last;
        slot.transition_left = self.transition_frames;
        Ok(())
    }

    pub fn note_off(&mut self, note: u8) -> Result<(), Error> {
        Self::validate_note(note)?;
        if self.params.legato {
            let was_top = self.held_len > 0 && self.held_notes[self.held_len - 1] == note;
            self.remove_held(note);
            if was_top {
                if self.held_len > 0 {
                    if let Some(index) = self.legato_slot() {
                        let previous = self.held_notes[self.held_len - 1];
                        let velocity = self.held_velocities[self.held_len - 1];
                        self.promote(index);
                        let slot = &mut self.slots[index];
                        slot.voice.note_on(midi_hz(previous), velocity)?;
                        slot.note = Some(previous);
                        slot.held = true;
                        slot.velocity = f32::from(velocity) / 127.0;
                        slot.transition_from = slot.last;
                        slot.transition_left = self.transition_frames;
                        return Ok(());
                    }
                } else if self.params.sustain {
                    if let Some(index) = self.legato_slot() {
                        self.slots[index].held = false;
                        self.slots[index].pedal = true;
                    }
                    return Ok(());
                }
            } else if !self
                .slots
                .iter()
                .any(|slot| slot.held && slot.note == Some(note))
            {
                return Ok(());
            }
        }
        for slot in &mut self.slots {
            if slot.held && slot.note == Some(note) {
                slot.held = false;
                if self.params.sustain {
                    slot.pedal = true;
                } else {
                    slot.pedal = false;
                    slot.voice.note_off();
                }
            }
        }
        Ok(())
    }

    /// Releases all held notes normally rather than truncating their tails.
    pub fn all_notes_off(&mut self) {
        self.clear_held();
        for slot in &mut self.slots {
            slot.held = false;
            slot.pedal = false;
            slot.voice.note_off();
        }
    }

    pub fn next_frame(&mut self) -> [f32; 2] {
        let mut mix = [0.0_f64; 2];
        for slot in &mut self.slots {
            // Idle voices still advance their free-running LFO and controls.
            let mut frame = slot.voice.next_frame_unclipped().map(|v| v * slot.velocity);
            if slot.transition_left > 0 {
                let progress = (self.transition_frames - slot.transition_left) as f64
                    / (self.transition_frames - 1) as f64;
                let weight = (progress * progress * (3.0 - 2.0 * progress)) as f32;
                for (channel, sample) in frame.iter_mut().enumerate() {
                    *sample = slot.transition_from[channel] * (1.0 - weight) + *sample * weight;
                }
                slot.transition_left -= 1;
            }
            slot.last = frame;
            for channel in 0..2 {
                mix[channel] += frame[channel] as f64;
            }
            if slot.voice.is_silent() && slot.transition_left == 0 {
                slot.note = None;
                slot.held = false;
                slot.pedal = false;
            }
        }
        mix.map(|v| (v / POLYPHONY as f64).clamp(-1.0, 1.0) as f32)
    }

    /// Overwrites caller-owned frames with the bounded stereo mix.
    pub fn render(&mut self, output: &mut [[f32; 2]]) {
        for frame in output {
            *frame = self.next_frame();
        }
    }

    /// Reports the most recently triggered active slot, including release tails.
    /// With no active slots, note sources are zero, channel mod wheel is the live
    /// parameter, and effective values are the base snapshot plus the wheel mix.
    /// Other modulation belongs to each voice, not to this display selection.
    pub fn telemetry(&self) -> Telemetry {
        self.slots
            .iter()
            .filter(|slot| slot.note.is_some())
            .min_by_key(|slot| slot.rank)
            .map(|slot| slot.voice.telemetry())
            .unwrap_or(self.idle_telemetry)
    }

    /// Includes held notes, release envelopes and unfinished trigger fades.
    pub fn active_voice_count(&self) -> usize {
        self.slots.iter().filter(|slot| slot.note.is_some()).count()
    }

    fn validate_note(note: u8) -> Result<(), Error> {
        if note > 127 {
            Err(Error::InvalidParameter("MIDI note"))
        } else {
            Ok(())
        }
    }

    fn legato_slot(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.held || slot.pedal)
            .min_by_key(|(_, slot)| slot.rank)
            .map(|(i, _)| i)
    }

    fn promote(&mut self, index: usize) {
        let old_rank = self.slots[index].rank;
        for slot in &mut self.slots {
            if slot.rank < old_rank {
                slot.rank += 1;
            }
        }
        self.slots[index].rank = 0;
    }

    fn capture_held_keys(&mut self) {
        self.clear_held();
        for rank in (0..POLYPHONY).rev() {
            if let Some(slot) = self
                .slots
                .iter()
                .find(|slot| slot.held && slot.rank == rank)
            {
                if let Some(note) = slot.note {
                    let velocity = (slot.velocity * 127.0).round() as u8;
                    self.push_held(note, velocity);
                }
            }
        }
        let keep = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.held)
            .min_by_key(|(_, slot)| slot.rank)
            .map(|(i, _)| i)
            .or_else(|| self.legato_slot());
        if let Some(keep) = keep {
            for (i, slot) in self.slots.iter_mut().enumerate() {
                if i != keep && (slot.held || slot.pedal) {
                    slot.held = false;
                    slot.pedal = false;
                    slot.voice.note_off();
                }
            }
            self.promote(keep);
        }
    }

    fn push_held(&mut self, note: u8, velocity: u8) {
        self.remove_held(note);
        if self.held_len < self.held_notes.len() {
            self.held_notes[self.held_len] = note;
            self.held_velocities[self.held_len] = velocity;
            self.held_len += 1;
        }
    }

    fn remove_held(&mut self, note: u8) {
        if let Some(index) = self.held_notes[..self.held_len]
            .iter()
            .position(|&held| held == note)
        {
            let last = self.held_len - 1;
            if index != last {
                self.held_notes.copy_within(index + 1..self.held_len, index);
                self.held_velocities
                    .copy_within(index + 1..self.held_len, index);
            }
            self.held_len = last;
        }
    }

    fn clear_held(&mut self) {
        self.held_len = 0;
    }

    fn release_unheld(&mut self) {
        for slot in &mut self.slots {
            if !slot.held && slot.note.is_some() {
                slot.pedal = false;
                slot.voice.note_off();
            }
        }
    }
}

fn midi_hz(note: u8) -> f64 {
    440.0 * 2.0_f64.powf((f64::from(note) - 69.0) / 12.0)
}
