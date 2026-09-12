use plasma_kernel::{POLYPHONY, PolySynth, VoiceParams, Waveform};

fn synth(seed: u64) -> PolySynth {
    let mut synth = PolySynth::new(48000.0, seed).unwrap();
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    synth.set_params(params).unwrap();
    synth
}

fn advance(synth: &mut PolySynth, frames: usize) {
    for _ in 0..frames {
        synth.next_frame();
    }
}

#[test]
fn selective_release_preserves_other_notes_and_release_tails() {
    let mut synth = synth(1);
    synth.note_on(60, 127).unwrap();
    synth.note_on(64, 127).unwrap();
    advance(&mut synth, 1000);
    synth.note_off(60).unwrap();
    assert_eq!(synth.active_voice_count(), 2);
    advance(&mut synth, 1000);
    assert_eq!(synth.active_voice_count(), 1);
    assert!(synth.telemetry().env > 0.99);
    synth.all_notes_off();
    assert_eq!(synth.active_voice_count(), 1);
    advance(&mut synth, 1000);
    assert_eq!(synth.active_voice_count(), 0);
    assert_eq!(synth.next_frame(), [0.0; 2]);
    assert_eq!(synth.telemetry().env, 0.0);
}

#[test]
fn repeated_held_note_retriggers_without_stacking_and_zero_velocity_releases() {
    let mut synth = synth(2);
    synth.note_on(60, 127).unwrap();
    advance(&mut synth, 1000);
    let previous = synth.next_frame();
    let env = synth.telemetry().env;
    synth.note_on(60, 24).unwrap();
    assert_eq!(synth.active_voice_count(), 1);
    assert_eq!(synth.telemetry().env, env);
    assert_eq!(synth.next_frame(), previous);
    synth.note_on(60, 0).unwrap();
    advance(&mut synth, 1000);
    assert_eq!(synth.active_voice_count(), 0);
}

#[test]
fn released_slot_is_stolen_before_an_older_held_note() {
    let mut synth = synth(3);
    for note in 60..68 {
        synth.note_on(note, 127).unwrap();
    }
    advance(&mut synth, 1000);
    synth.note_off(67).unwrap();
    synth.note_on(68, 127).unwrap();
    synth.note_off(67).unwrap(); // Stale release cannot affect replacement.
    advance(&mut synth, 1000);
    assert_eq!(synth.active_voice_count(), POLYPHONY);
    synth.note_off(60).unwrap(); // Oldest held note survived allocation.
    advance(&mut synth, 1000);
    assert_eq!(synth.active_voice_count(), POLYPHONY - 1);
    synth.note_off(68).unwrap();
    advance(&mut synth, 1000);
    assert_eq!(synth.active_voice_count(), POLYPHONY - 2);
}

#[test]
fn oldest_held_slot_is_stolen_and_retrigger_refreshes_its_age() {
    let mut synth = synth(4);
    for note in 60..68 {
        synth.note_on(note, 127).unwrap();
    }
    advance(&mut synth, 1000);
    synth.note_on(60, 100).unwrap();
    synth.note_on(68, 127).unwrap(); // 61, not the retriggered 60, is oldest.
    synth.note_off(61).unwrap();
    advance(&mut synth, 1000);
    assert_eq!(synth.active_voice_count(), POLYPHONY);
    synth.note_off(60).unwrap();
    advance(&mut synth, 1000);
    assert_eq!(synth.active_voice_count(), POLYPHONY - 1);
}

#[test]
fn velocity_is_linear_and_invalid_events_leave_audio_unchanged() {
    let mut loud = synth(5);
    let mut quiet = synth(5);
    loud.note_on(69, 127).unwrap();
    quiet.note_on(69, 32).unwrap();
    let mut energy = 0.0_f64;
    for _ in 0..2000 {
        let a = loud.next_frame();
        let b = quiet.next_frame();
        for channel in 0..2 {
            assert!((b[channel] - a[channel] * (32.0 / 127.0)).abs() < 1e-6);
            energy += (a[channel] as f64).powi(2);
        }
    }
    assert!(energy > 0.01);
    let mut reference = synth(6);
    let mut rejected = synth(6);
    reference.note_on(60, 127).unwrap();
    rejected.note_on(60, 127).unwrap();
    assert!(rejected.note_on(128, 127).is_err());
    assert!(rejected.note_on(60, 128).is_err());
    assert!(rejected.note_off(255).is_err());
    let mut invalid = VoiceParams::default();
    invalid.volume = f32::NAN;
    assert!(rejected.set_params(invalid).is_err());
    for _ in 0..1000 {
        assert_eq!(reference.next_frame(), rejected.next_frame());
    }
}

#[test]
fn repeated_steals_restart_from_actual_output_and_fade_finishes() {
    let mut synth = synth(7);
    for note in 60..68 {
        synth.note_on(note, 127).unwrap();
    }
    advance(&mut synth, 1000);
    let mut previous = synth.next_frame();
    for iteration in 0..64 {
        // Replace every slot, including slots whose preceding fade is unfinished.
        let start = if iteration % 2 == 0 { 72 } else { 60 };
        for note in start..start + 8 {
            synth.note_on(note, 127).unwrap();
        }
        assert_eq!(synth.next_frame(), previous);
        previous = synth.next_frame();
    }
    synth.all_notes_off();
    advance(&mut synth, 1000);
    assert_eq!(synth.active_voice_count(), 0);
    assert_eq!(synth.next_frame(), [0.0; 2]);
}

#[test]
fn full_polyphonic_mix_stays_finite_and_bounded_under_modulation() {
    for sample_rate in [8000.0, 48000.0, 96000.0] {
        let mut synth = PolySynth::new(sample_rate, 8).unwrap();
        let mut params = VoiceParams::default();
        params.volume = 1.0;
        params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.001, 30.0, 0.0, 20000.0, 1.0]);
        for osc in &mut params.oscillators {
            osc.level = 1.0;
            osc.unison = 4;
            osc.waveform = Waveform::Pulse;
            osc.pulse_width = 0.01;
        }
        params.routes[1][34] = 1.0;
        synth.set_params(params).unwrap();
        for note in 60..68 {
            synth.note_on(note, 127).unwrap();
        }
        for frame in 0..12000 {
            if frame % 31 == 0 {
                synth.note_on(36 + ((frame / 31) % 80) as u8, 127).unwrap();
            }
            assert!(
                synth
                    .next_frame()
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= 1.0)
            );
        }
        synth.all_notes_off();
        advance(&mut synth, 2000);
        assert_eq!(synth.next_frame(), [0.0; 2]);
    }
}

#[test]
fn mod_envelopes_are_per_voice_and_reassigned_slots_start_fresh() {
    let mut synth = PolySynth::new(48000.0, 13).unwrap();
    let mut params = VoiceParams::default();
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.001]);
    params.globals[8..].copy_from_slice(&[1.0, 0.1, 1.0, 10.0]);
    params.routes[2][34] = -0.5;
    synth.set_params(params).unwrap();
    synth.note_on(60, 127).unwrap();
    advance(&mut synth, 4800);
    assert!((synth.telemetry().mod_env - 0.1).abs() < 0.001);
    synth.note_on(64, 127).unwrap();
    assert_eq!(synth.telemetry().mod_env, 0.0);
    advance(&mut synth, 2400);
    assert!((synth.telemetry().mod_env - 0.05).abs() < 0.001);
    synth.note_off(64).unwrap();
    advance(&mut synth, 240);
    assert_eq!(synth.active_voice_count(), 1);
    assert!((synth.telemetry().mod_env - 0.155).abs() < 0.001);
    assert!(synth.telemetry().effective[34] < params.normalized()[34] - 0.07);
    synth.note_on(67, 127).unwrap();
    assert_eq!(synth.telemetry().mod_env, 0.0);
    advance(&mut synth, 240);
    assert!((synth.telemetry().mod_env - 0.005).abs() < 0.001);
}

#[test]
fn long_mod_release_does_not_retain_amp_silent_slots() {
    let mut synth = PolySynth::new(48000.0, 14).unwrap();
    let mut params = VoiceParams::default();
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.001]);
    params.globals[8..].copy_from_slice(&[0.001, 0.001, 1.0, 10.0]);
    params.routes[2][34] = -0.5;
    synth.set_params(params).unwrap();
    for note in 60..68 {
        synth.note_on(note, 127).unwrap();
    }
    advance(&mut synth, 1000);
    assert!(synth.telemetry().mod_env > 0.99);
    synth.all_notes_off();
    advance(&mut synth, 240);
    assert_eq!(synth.active_voice_count(), 0);
    assert_eq!(synth.next_frame(), [0.0; 2]);
    assert_eq!(synth.telemetry().mod_env, 0.0);
    synth.note_on(72, 127).unwrap();
    assert_eq!(synth.telemetry().mod_env, 0.0);
    advance(&mut synth, 1000);
    assert_eq!(synth.active_voice_count(), 1);
    assert!(synth.telemetry().mod_env > 0.99);
}
