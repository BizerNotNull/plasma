//! Fixed-capacity MIDI voice allocation and post-voice mixing.

use crate::{Error, Telemetry, Voice, VoiceParams, check_sample_rate};

pub const POLYPHONY: usize = 8;

struct Slot {
    voice: Voice,
    note: Option<u8>,
    held: bool,
    // A permutation of 0..POLYPHONY; zero is most recently triggered.
    rank: usize,
    velocity: f32,
    last: [f32; 2],
    transition_from: [f32; 2],
    transition_left: usize,
}

/// Eight independently seeded oscillator/envelope/LFO/filter voices.
///
/// Repeated held notes retrigger their existing slot. Otherwise allocation uses
/// an idle slot, then the oldest released slot, then the oldest held slot.
/// Release tails count as active until their envelope and transition finish.
/// Age is a bounded rank permutation, so arbitrarily long event streams cannot
/// overflow a timestamp or change the stealing order.
///
/// Each trigger crossfades from the slot's last emitted sample to its new live
/// signal over 3 ms (at least two frames). This is a bounded sample-hold fade,
/// not a second oscillator voice: even repeated steals restart from the actual
/// last output, with no accumulated correction or extra rendering work. The
/// first frame after a trigger exactly preserves that slot's preceding output.
/// Velocity scales amplitude linearly by velocity / 127, before this fade.
///
/// Mixing uses fixed 1/8 headroom, independent of the active count, then clamps
/// only the final stereo sum to [-1, 1]. Construction, events and rendering use
/// fixed storage and never allocate, lock or wait.
pub struct PolySynth {
    slots: [Slot; POLYPHONY],
    transition_frames: usize,
    idle_telemetry: Telemetry,
    params: VoiceParams,
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
                rank: i,
                velocity: 0.0,
                last: [0.0; 2],
                transition_from: [0.0; 2],
                transition_left: 0,
            }),
            transition_frames: (sample_rate * 0.003).round().max(2.0) as usize,
            idle_telemetry: Telemetry::default(),
            params: VoiceParams::default(),
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
        self.idle_telemetry.effective = params.normalized();
        self.params = params;
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
        let repeated = self
            .slots
            .iter()
            .position(|slot| slot.held && slot.note == Some(note));
        let index = repeated
            .or_else(|| self.slots.iter().position(|slot| slot.note.is_none()))
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
            });
        let old_rank = self.slots[index].rank;
        for slot in &mut self.slots {
            if slot.rank < old_rank {
                slot.rank += 1;
            }
        }
        let slot = &mut self.slots[index];
        slot.rank = 0;
        if repeated.is_none() {
            slot.voice.reset_note();
        }
        slot.voice
            .note_on(440.0 * 2.0_f64.powf((note as f64 - 69.0) / 12.0))?;
        slot.note = Some(note);
        slot.held = true;
        slot.velocity = velocity as f32 / 127.0;
        slot.transition_from = slot.last;
        slot.transition_left = self.transition_frames;
        Ok(())
    }

    pub fn note_off(&mut self, note: u8) -> Result<(), Error> {
        Self::validate_note(note)?;
        for slot in &mut self.slots {
            if slot.held && slot.note == Some(note) {
                slot.held = false;
                slot.voice.note_off();
            }
        }
        Ok(())
    }

    /// Releases all held notes normally rather than truncating their tails.
    pub fn all_notes_off(&mut self) {
        for slot in &mut self.slots {
            slot.held = false;
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
    /// With no active slots, ENV/LFO are zero and effective values are the base
    /// snapshot. Modulation belongs to each voice, not to this display selection.
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
}
