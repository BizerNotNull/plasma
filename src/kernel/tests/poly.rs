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

#[test]
fn chord_audio_keeps_each_notes_sources_through_release() {
    let seed = 31_u64;
    let mut chord = PolySynth::new(48000.0, seed).unwrap();
    let mut low = PolySynth::new(48000.0, seed).unwrap();
    // Match the independent oscillator seed used by the chord's second slot.
    let mut high = PolySynth::new(48000.0, seed.wrapping_add(0x9e3779b97f4a7c15)).unwrap();
    let mut params = VoiceParams::default();
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    params.globals[6] = 800.0;
    params.routes[3][34] = 0.3;
    params.routes[4][7] = 0.8;
    for synth in [&mut chord, &mut low, &mut high] {
        synth.set_params(params).unwrap();
    }
    chord.note_on(36, 32).unwrap();
    chord.note_on(84, 127).unwrap();
    low.note_on(36, 32).unwrap();
    high.note_on(84, 127).unwrap();
    for frame in 0..12000 {
        if frame == 2000 {
            chord.note_off(84).unwrap();
            high.note_off(84).unwrap();
        }
        let actual = chord.next_frame();
        let a = low.next_frame();
        let b = high.next_frame();
        for channel in 0..2 {
            assert!((actual[channel] - (a[channel] + b[channel])).abs() < 1e-7);
        }
        if frame == 3000 {
            let t = chord.telemetry();
            assert_eq!(t.velocity, 1.0);
            assert!((t.key_track - 0.4).abs() < 1e-6);
            assert!((t.effective[7] - 0.82).abs() < 1e-6);
        }
    }
    assert_eq!(chord.active_voice_count(), 1);
    let t = chord.telemetry();
    assert_eq!(t.velocity, 32.0 / 127.0);
    assert!((t.key_track + 0.4).abs() < 1e-6);
    assert!((t.effective[7] - 0.18).abs() < 1e-6);
}

#[test]
fn retriggers_and_stolen_slots_use_new_note_sources_immediately() {
    let mut synth = PolySynth::new(48000.0, 32).unwrap();
    let mut params = VoiceParams::default();
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.01]);
    params.routes[3][2] = 0.5;
    params.routes[4][7] = 0.5;
    synth.set_params(params).unwrap();
    for note in 36..44 {
        synth.note_on(note, 127).unwrap();
    }
    advance(&mut synth, 1000);
    synth.note_on(43, 16).unwrap(); // Held-note retrigger, same slot.
    assert_eq!(synth.active_voice_count(), POLYPHONY);
    assert!((synth.telemetry().effective[2] - 0.5 * 16.0 / 127.0).abs() < 1e-6);
    synth.note_on(84, 64).unwrap(); // Steals oldest held note, 36.
    let stolen = synth.telemetry();
    assert_eq!(stolen.velocity, 64.0 / 127.0);
    assert!((stolen.key_track - 0.4).abs() < 1e-6);
    assert!((stolen.effective[2] - 0.5 * 64.0 / 127.0).abs() < 1e-6);
    assert!((stolen.effective[7] - 0.7).abs() < 1e-6);
    synth.note_off(84).unwrap();
    synth.note_on(48, 1).unwrap(); // Prefers the released slot over held notes.
    let replaced = synth.telemetry();
    assert_eq!(replaced.velocity, 1.0 / 127.0);
    assert!((replaced.key_track + 0.2).abs() < 1e-6);
    assert!((replaced.effective[2] - 0.5 / 127.0).abs() < 1e-6);
    assert!((replaced.effective[7] - 0.4).abs() < 1e-6);
    synth.all_notes_off();
    advance(&mut synth, 1000);
    assert_eq!(synth.active_voice_count(), 0);
    assert_eq!(synth.telemetry().velocity, 0.0);
    assert_eq!(synth.telemetry().key_track, 0.0);
    synth.note_on(60, 32).unwrap(); // Reuses an idle slot.
    assert_eq!(synth.telemetry().velocity, 32.0 / 127.0);
    assert!(synth.telemetry().key_track.abs() < 1e-6);
    assert!((synth.telemetry().effective[7] - 0.5).abs() < 1e-6);
}

#[test]
fn legato_reuses_one_voice_and_poly_is_unchanged_when_legato_off() {
    let mut poly = synth(40);
    for note in 60..68 {
        poly.note_on(note, 127).unwrap();
    }
    assert_eq!(poly.active_voice_count(), POLYPHONY);

    let mut legato = synth(41);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.legato = true;
    params.glide = 0.05;
    legato.set_params(params).unwrap();
    legato.note_on(60, 127).unwrap();
    legato.note_on(64, 127).unwrap();
    assert_eq!(legato.active_voice_count(), 1);
}

#[test]
fn legato_release_returns_to_previous_held_note() {
    let mut legato = synth(42);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.legato = true;
    legato.set_params(params).unwrap();
    legato.note_on(60, 1).unwrap();
    advance(&mut legato, 100);
    assert!(legato.telemetry().key_track.abs() < 1e-6);
    assert!((legato.telemetry().velocity - 1.0 / 127.0).abs() < 1e-6);

    legato.note_on(64, 127).unwrap();
    advance(&mut legato, 100);
    assert_eq!(legato.active_voice_count(), 1);
    assert!((legato.telemetry().key_track - 4.0 / 60.0).abs() < 1e-5);

    legato.note_off(64).unwrap();
    advance(&mut legato, 100);
    assert_eq!(legato.active_voice_count(), 1);
    assert!(legato.telemetry().key_track.abs() < 1e-6);
    assert!((legato.telemetry().velocity - 1.0 / 127.0).abs() < 1e-6);

    legato.note_off(60).unwrap();
    advance(&mut legato, 2000);
    assert_eq!(legato.active_voice_count(), 0);

    legato.note_on(60, 127).unwrap();
    legato.note_on(64, 127).unwrap();
    legato.note_off(60).unwrap();
    advance(&mut legato, 100);
    assert_eq!(legato.active_voice_count(), 1);
    assert!((legato.telemetry().key_track - 4.0 / 60.0).abs() < 1e-5);
    legato.note_off(64).unwrap();
    advance(&mut legato, 2000);
    assert_eq!(legato.active_voice_count(), 0);
}

#[test]
fn legato_stack_walks_back_through_three_held_notes() {
    let mut legato = synth(43);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.legato = true;
    legato.set_params(params).unwrap();
    legato.note_on(60, 127).unwrap();
    legato.note_on(64, 127).unwrap();
    legato.note_on(67, 127).unwrap();
    assert_eq!(legato.active_voice_count(), 1);
    assert!((legato.telemetry().key_track - 7.0 / 60.0).abs() < 1e-5);
    legato.note_off(67).unwrap();
    assert!((legato.telemetry().key_track - 4.0 / 60.0).abs() < 1e-5);
    legato.note_off(64).unwrap();
    assert!(legato.telemetry().key_track.abs() < 1e-6);
    assert_eq!(legato.active_voice_count(), 1);
}

#[test]
fn disabling_legato_clears_held_stack_and_releases_normally() {
    let mut poly = synth(44);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.legato = true;
    poly.set_params(params).unwrap();
    poly.note_on(60, 127).unwrap();
    poly.note_on(64, 127).unwrap();
    params.legato = false;
    poly.set_params(params).unwrap();
    poly.note_off(64).unwrap();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 0);
    params.legato = true;
    poly.set_params(params).unwrap();
    poly.note_on(67, 127).unwrap();
    poly.note_off(67).unwrap();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 0);
}

#[test]
fn all_notes_off_forgets_legato_stack() {
    let mut poly = synth(45);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.legato = true;
    poly.set_params(params).unwrap();
    poly.note_on(60, 127).unwrap();
    poly.note_on(64, 127).unwrap();
    poly.all_notes_off();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 0);
    poly.note_on(67, 127).unwrap();
    poly.note_off(67).unwrap();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 0);
}

#[test]
fn enabling_legato_captures_held_key_for_last_note_priority() {
    let mut poly = synth(46);
    poly.note_on(60, 1).unwrap();
    advance(&mut poly, 100);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.legato = true;
    poly.set_params(params).unwrap();
    poly.note_on(64, 127).unwrap();
    advance(&mut poly, 100);
    assert_eq!(poly.active_voice_count(), 1);
    poly.note_off(64).unwrap();
    advance(&mut poly, 100);
    assert_eq!(poly.active_voice_count(), 1);
    assert!(poly.telemetry().key_track.abs() < 1e-6);
    assert!((poly.telemetry().velocity - 1.0 / 127.0).abs() < 1e-6);
}

#[test]
fn enabling_legato_collapses_chord_and_walks_captured_stack() {
    let mut poly = synth(47);
    poly.note_on(60, 127).unwrap();
    poly.note_on(64, 127).unwrap();
    assert_eq!(poly.active_voice_count(), 2);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.legato = true;
    poly.set_params(params).unwrap();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 1);
    assert!((poly.telemetry().key_track - 4.0 / 60.0).abs() < 1e-5);
    poly.note_off(64).unwrap();
    advance(&mut poly, 100);
    assert_eq!(poly.active_voice_count(), 1);
    assert!(poly.telemetry().key_track.abs() < 1e-6);
}

#[test]
fn channel_pitch_bend_shifts_every_voice_without_dropping_notes() {
    let mut plain = synth(5);
    let mut bent = synth(5);
    let mut params = VoiceParams::default();
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    plain.set_params(params).unwrap();
    params.pitch_bend = 1.0;
    params.pitch_bend_range = 12.0;
    bent.set_params(params).unwrap();
    for note in [60_u8, 64, 67] {
        plain.note_on(note, 127).unwrap();
        bent.note_on(note, 127).unwrap();
    }
    assert_eq!(plain.active_voice_count(), 3);
    assert_eq!(bent.active_voice_count(), 3);
    let mut a = [[0.0; 2]; 512];
    let mut b = [[0.0; 2]; 512];
    plain.render(&mut a);
    bent.render(&mut b);
    assert_ne!(a, b);
    assert!(b.iter().all(|f| f.iter().all(|s| s.is_finite())));
    assert_eq!(bent.active_voice_count(), 3);
}

#[test]
fn sustain_holds_unheld_notes_until_pedal_is_released() {
    let mut poly = synth(50);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.sustain = true;
    poly.set_params(params).unwrap();
    poly.note_on(60, 127).unwrap();
    poly.note_on(64, 127).unwrap();
    advance(&mut poly, 200);
    poly.note_off(60).unwrap();
    poly.note_off(64).unwrap();
    advance(&mut poly, 200);
    assert_eq!(poly.active_voice_count(), 2);
    assert!(poly.telemetry().env > 0.5);
    assert!((0..512).any(|_| poly.next_frame().iter().any(|s| s.abs() > 0.0001)));

    params.sustain = false;
    poly.set_params(params).unwrap();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 0);
}

#[test]
fn releasing_sustain_keeps_physically_held_notes() {
    let mut poly = synth(51);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.sustain = true;
    poly.set_params(params).unwrap();
    poly.note_on(60, 127).unwrap();
    poly.note_on(64, 127).unwrap();
    poly.note_off(64).unwrap();
    params.sustain = false;
    poly.set_params(params).unwrap();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 1);
    assert!(poly.telemetry().key_track.abs() < 1e-6);
    poly.note_off(60).unwrap();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 0);
}

#[test]
fn pedaled_note_retriggers_its_slot_and_is_stolen_before_held() {
    let mut poly = synth(52);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.sustain = true;
    poly.set_params(params).unwrap();
    for note in 60..68 {
        poly.note_on(note, 127).unwrap();
    }
    poly.note_off(67).unwrap();
    poly.note_on(67, 100).unwrap();
    assert_eq!(poly.active_voice_count(), POLYPHONY);
    poly.note_off(67).unwrap();
    poly.note_on(68, 127).unwrap();
    poly.note_off(60).unwrap();
    advance(&mut poly, 200);
    assert_eq!(poly.active_voice_count(), POLYPHONY);
    params.sustain = false;
    poly.set_params(params).unwrap();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), POLYPHONY - 1);
}

#[test]
fn all_notes_off_releases_despite_sustain() {
    let mut poly = synth(53);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.sustain = true;
    poly.set_params(params).unwrap();
    poly.note_on(60, 127).unwrap();
    poly.note_on(64, 127).unwrap();
    poly.note_off(60).unwrap();
    poly.all_notes_off();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 0);
}

#[test]
fn legato_last_key_stays_sounding_while_sustained() {
    let mut poly = synth(54);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.legato = true;
    params.sustain = true;
    poly.set_params(params).unwrap();
    poly.note_on(60, 127).unwrap();
    poly.note_on(64, 127).unwrap();
    poly.note_off(64).unwrap();
    assert!(poly.telemetry().key_track.abs() < 1e-6);
    poly.note_off(60).unwrap();
    advance(&mut poly, 200);
    assert_eq!(poly.active_voice_count(), 1);
    assert!(poly.telemetry().env > 0.5);
    assert!((0..512).any(|_| poly.next_frame().iter().any(|s| s.abs() > 0.0001)));
    poly.note_on(67, 127).unwrap();
    assert_eq!(poly.active_voice_count(), 1);
    assert!((poly.telemetry().key_track - 7.0 / 60.0).abs() < 1e-5);
}

#[test]
fn pedaled_retrigger_reuses_slot_without_stealing_held() {
    let mut poly = synth(55);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.sustain = true;
    poly.set_params(params).unwrap();
    poly.note_on(60, 127).unwrap();
    poly.note_on(64, 1).unwrap();
    poly.note_off(64).unwrap();
    poly.note_on(64, 127).unwrap();
    assert_eq!(poly.active_voice_count(), 2);
    poly.note_off(60).unwrap();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 2);
    assert!((poly.telemetry().key_track - 4.0 / 60.0).abs() < 1e-5);
    assert!((poly.telemetry().velocity - 1.0).abs() < 1e-6);
}

#[test]
fn enabling_legato_collapses_pedaled_voices_and_prefers_held() {
    let mut poly = synth(56);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.sustain = true;
    poly.set_params(params).unwrap();
    poly.note_on(60, 1).unwrap();
    poly.note_on(64, 127).unwrap();
    poly.note_off(64).unwrap();
    assert_eq!(poly.active_voice_count(), 2);
    params.legato = true;
    poly.set_params(params).unwrap();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 1);
    assert!(poly.telemetry().key_track.abs() < 1e-6);
    assert!((poly.telemetry().velocity - 1.0 / 127.0).abs() < 1e-6);
    poly.note_off(60).unwrap();
    advance(&mut poly, 200);
    assert_eq!(poly.active_voice_count(), 1);
    assert!(poly.telemetry().env > 0.5);
}

#[test]
fn enabling_legato_with_only_pedaled_voices_keeps_one() {
    let mut poly = synth(57);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.sustain = true;
    poly.set_params(params).unwrap();
    poly.note_on(60, 127).unwrap();
    poly.note_on(64, 127).unwrap();
    poly.note_off(60).unwrap();
    poly.note_off(64).unwrap();
    assert_eq!(poly.active_voice_count(), 2);
    params.legato = true;
    poly.set_params(params).unwrap();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 1);
    assert!((poly.telemetry().key_track - 4.0 / 60.0).abs() < 1e-5);
    poly.note_on(67, 127).unwrap();
    assert_eq!(poly.active_voice_count(), 1);
    assert!((poly.telemetry().key_track - 7.0 / 60.0).abs() < 1e-5);
}

#[test]
fn channel_mod_wheel_is_shared_and_does_not_drop_notes() {
    let mut poly = synth(58);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.routes[5][0] = 1.0;
    poly.set_params(params).unwrap();
    for note in [60_u8, 64, 67] {
        poly.note_on(note, 127).unwrap();
    }
    advance(&mut poly, 200);
    assert_eq!(poly.active_voice_count(), 3);
    assert_eq!(poly.telemetry().mod_wheel, 0.0);
    let mut before = [[0.0; 2]; 128];
    poly.render(&mut before);
    params.mod_wheel = 1.0;
    poly.set_params(params).unwrap();
    assert_eq!(poly.active_voice_count(), 3);
    assert_eq!(poly.telemetry().mod_wheel, 1.0);
    let mut after = [[0.0; 2]; 128];
    poly.render(&mut after);
    assert_ne!(before, after);
    assert!(after.iter().flatten().all(|s| s.is_finite()));
    poly.all_notes_off();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 0);
    assert_eq!(poly.telemetry().mod_wheel, 1.0);
    let expected = (params.normalized()[0] + params.routes[5][0] * params.mod_wheel).clamp(0.0, 1.0);
    assert!((poly.telemetry().effective[0] - expected).abs() < 1e-6);
    params.routes[5][52] = 1.0;
    poly.set_params(params).unwrap();
    let expected_end = (params.normalized()[52] + params.mod_wheel).clamp(0.0, 1.0);
    assert!((poly.telemetry().effective[52] - expected_end).abs() < 1e-6);
    assert_eq!(poly.active_voice_count(), 0);
}

#[test]
fn channel_aftertouch_is_shared_and_does_not_drop_notes() {
    let mut poly = synth(58);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.routes[6][0] = 1.0;
    poly.set_params(params).unwrap();
    for note in [60_u8, 64, 67] {
        poly.note_on(note, 127).unwrap();
    }
    advance(&mut poly, 200);
    assert_eq!(poly.active_voice_count(), 3);
    assert_eq!(poly.telemetry().aftertouch, 0.0);
    let mut before = [[0.0; 2]; 128];
    poly.render(&mut before);
    params.aftertouch = 1.0;
    poly.set_params(params).unwrap();
    assert_eq!(poly.active_voice_count(), 3);
    assert_eq!(poly.telemetry().aftertouch, 1.0);
    let mut after = [[0.0; 2]; 128];
    poly.render(&mut after);
    assert_ne!(before, after);
    assert!(after.iter().flatten().all(|s| s.is_finite()));
    poly.all_notes_off();
    advance(&mut poly, 2000);
    assert_eq!(poly.active_voice_count(), 0);
    assert_eq!(poly.telemetry().aftertouch, 1.0);
    let expected =
        (params.normalized()[0] + params.routes[6][0] * params.aftertouch).clamp(0.0, 1.0);
    assert!((poly.telemetry().effective[0] - expected).abs() < 1e-6);
    params.routes[6][52] = 1.0;
    poly.set_params(params).unwrap();
    let expected_end = (params.normalized()[52] + params.aftertouch).clamp(0.0, 1.0);
    assert!((poly.telemetry().effective[52] - expected_end).abs() < 1e-6);
    assert_eq!(poly.active_voice_count(), 0);
}

#[test]
fn always_glide_slides_staccato_from_last_pitch() {
    let mut fingered = synth(59);
    let mut always = synth(59);
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.01, 1.0, 0.0, 18000.0, 0.1]);
    params.glide = 0.1;
    fingered.set_params(params).unwrap();
    params.always_glide = true;
    always.set_params(params).unwrap();

    fingered.note_on(60, 127).unwrap();
    always.note_on(60, 127).unwrap();
    advance(&mut fingered, 500);
    advance(&mut always, 500);
    fingered.note_off(60).unwrap();
    always.note_off(60).unwrap();
    advance(&mut fingered, 2000);
    advance(&mut always, 2000);
    assert_eq!(fingered.active_voice_count(), 0);
    assert_eq!(always.active_voice_count(), 0);

    fingered.note_on(72, 127).unwrap();
    always.note_on(72, 127).unwrap();
    let mut a = [[0.0; 2]; 256];
    let mut b = [[0.0; 2]; 256];
    fingered.render(&mut a);
    always.render(&mut b);
    assert_ne!(a, b);
    assert!(b.iter().flatten().all(|s| s.is_finite()));
    assert_eq!(always.active_voice_count(), 1);
}
