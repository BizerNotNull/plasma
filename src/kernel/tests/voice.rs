use plasma_kernel::{FilterMode, LfoWave, TARGET_COUNT, Voice, VoiceParams, Waveform};

#[test]
fn release_survives_old_gate_and_retrigger_preserves_envelope_level() {
    let mut voice = Voice::new(48000.0, 7).unwrap();
    let mut params = VoiceParams::default();
    params.globals[..8].copy_from_slice(&[0.001, 0.001, 1.0, 0.2, 1.0, 0.0, 18000.0, 0.1]);
    voice.set_params(params).unwrap();
    voice.note_on(440.0, 127).unwrap();
    for _ in 0..1000 {
        voice.next_frame();
    }
    voice.note_off();
    for _ in 0..480 {
        voice.next_frame();
    }
    assert!(voice.telemetry().env > 0.9);
    let before = voice.telemetry().env;
    voice.note_on(440.0, 127).unwrap();
    assert_eq!(voice.telemetry().env, before);
    voice.note_off();
    for _ in 0..10000 {
        voice.next_frame();
    }
    assert!(voice.is_silent());
    assert_eq!(voice.next_frame(), [0.0; 2]);
}

#[test]
fn free_lfo_runs_while_idle_and_retrigger_resets_phase() {
    let mut voice = Voice::new(48000.0, 1).unwrap();
    let mut p = VoiceParams::default();
    p.lfo_wave = LfoWave::Saw;
    p.lfo_retrigger = false;
    p.globals[4] = 1.0;
    voice.set_params(p).unwrap();
    for _ in 0..12000 {
        voice.next_frame();
    }
    voice.note_on(220.0, 127).unwrap();
    voice.next_frame();
    assert!((voice.telemetry().lfo + 0.5).abs() < 0.001);
    p.lfo_retrigger = true;
    p.globals[5] = 0.25;
    voice.set_params(p).unwrap();
    voice.note_on(220.0, 127).unwrap();
    voice.next_frame();
    assert!((voice.telemetry().lfo + 0.5).abs() < 0.001);
    voice.note_on(220.0, 127).unwrap();
    voice.next_frame();
    assert!((voice.telemetry().lfo + 0.5).abs() < 0.001);
}

#[test]
fn dual_routes_clamp_without_changing_bases_and_rejection_is_atomic() {
    let mut voice = Voice::new(48000.0, 5).unwrap();
    let mut p = VoiceParams::default();
    p.globals[0] = 0.001;
    p.globals[2] = 1.0;
    p.lfo_wave = LfoWave::Square;
    p.routes[0] = [1.0; TARGET_COUNT];
    p.routes[1] = [1.0; TARGET_COUNT];
    voice.set_params(p).unwrap();
    voice.note_on(440.0, 127).unwrap();
    for _ in 0..100 {
        voice.next_frame();
    }
    let t = voice.telemetry();
    assert!(t.effective.iter().all(|v| (0.0..=1.0).contains(v)));
    assert_eq!(t.effective[27], 1.0);
    assert_eq!(voice.params(), &p);
    let mut invalid = p;
    invalid.routes[1][35] = f32::NAN;
    assert!(voice.set_params(invalid).is_err());
    assert_eq!(voice.params(), &p);
    invalid = p;
    invalid.globals[6] = 0.0;
    assert!(voice.set_params(invalid).is_err());
    assert_eq!(voice.params(), &p);
    p.routes[0] = [0.0; TARGET_COUNT];
    p.routes[1] = [0.0; TARGET_COUNT];
    voice.set_params(p).unwrap();
    voice.next_frame();
    assert_eq!(voice.telemetry().effective[27], 0.25);
}

#[test]
fn resonant_filter_remains_finite_during_extreme_cutoff_sweeps() {
    for sample_rate in [8000.0, 44100.0, 96000.0] {
        let mut voice = Voice::new(sample_rate, 3).unwrap();
        let mut p = VoiceParams::default();
        p.oscillators[0].waveform = Waveform::Pulse;
        p.oscillators[0].pulse_width = 0.01;
        p.globals[0] = 0.001;
        p.globals[2] = 1.0;
        p.globals[7] = 1.0;
        p.globals[4] = 30.0;
        p.lfo_wave = LfoWave::Square;
        p.routes[1][34] = 1.0;
        voice.set_params(p).unwrap();
        voice.note_on(110.0, 127).unwrap();
        for _ in 0..96000 {
            assert!(
                voice
                    .next_frame()
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= 1.0)
            );
        }
        voice.note_off();
        for _ in 0..96000 {
            voice.next_frame();
        }
        assert_eq!(voice.next_frame(), [0.0; 2]);
    }
}

#[test]
fn mod_envelope_changes_timbre_only_when_routed_without_changing_amp() {
    let mut fast = Voice::new(48000.0, 11).unwrap();
    let mut slow = Voice::new(48000.0, 11).unwrap();
    let mut params = VoiceParams::default();
    params.oscillators[0].waveform = Waveform::Saw;
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    params.globals[6] = 100.0;
    params.globals[8..].copy_from_slice(&[0.001, 0.001, 1.0, 0.001]);
    fast.set_params(params).unwrap();
    let mut slow_params = params;
    slow_params.globals[8..].copy_from_slice(&[1.0, 1.0, 0.2, 10.0]);
    slow.set_params(slow_params).unwrap();
    fast.note_on(220.0, 127).unwrap();
    slow.note_on(220.0, 127).unwrap();
    for _ in 0..4800 {
        assert_eq!(fast.next_frame(), slow.next_frame());
    }
    assert!(fast.telemetry().mod_env > 0.99);
    assert!(slow.telemetry().mod_env < 0.11);

    params.routes[2][34] = 0.7;
    slow_params.routes[2][34] = 0.7;
    fast.set_params(params).unwrap();
    slow.set_params(slow_params).unwrap();
    let mut fast_energy = 0.0_f64;
    let mut slow_energy = 0.0_f64;
    for _ in 0..4800 {
        fast_energy += (fast.next_frame()[0] as f64).powi(2);
        slow_energy += (slow.next_frame()[0] as f64).powi(2);
        assert_eq!(fast.telemetry().env, slow.telemetry().env);
    }
    assert!(fast_energy > slow_energy * 2.0);
    fast.note_off();
    slow.note_off();
    for _ in 0..480 {
        fast.next_frame();
        slow.next_frame();
    }
    assert_eq!(fast.telemetry().mod_env, 0.0);
    assert!(slow.telemetry().mod_env > 0.19);
    assert_eq!(fast.telemetry().env, slow.telemetry().env);
    assert!(fast.telemetry().env > 0.9);
    let before = slow.telemetry().mod_env;
    slow.note_on(220.0, 127).unwrap();
    assert_eq!(slow.telemetry().mod_env, before);
    slow.next_frame();
    assert!(slow.telemetry().mod_env > before);
}

#[test]
fn mod_envelope_self_route_uses_previous_control_tick() {
    let mut voice = Voice::new(8000.0, 12).unwrap();
    let mut params = VoiceParams::default();
    params.routes[2][36] = 0.5;
    voice.set_params(params).unwrap();
    voice.note_on(220.0, 127).unwrap();
    let base = params.normalized()[36];
    for _ in 0..8 {
        voice.next_frame();
        assert_eq!(voice.telemetry().effective[36], base);
    }
    let previous = voice.telemetry().mod_env;
    voice.next_frame();
    assert!((voice.telemetry().effective[36] - (base + previous * 0.5)).abs() < 1e-6);
    assert!(voice.telemetry().mod_env > previous);
}

#[test]
fn note_sources_change_audio_only_when_routed() {
    let mut params = VoiceParams::default();
    params.oscillators[0].waveform = Waveform::Saw;
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    params.globals[6] = 100.0;
    let mut loud = Voice::new(48000.0, 21).unwrap();
    let mut quiet = Voice::new(48000.0, 21).unwrap();
    loud.set_params(params).unwrap();
    quiet.set_params(params).unwrap();
    loud.note_on(440.0, 127).unwrap();
    quiet.note_on(440.0, 0).unwrap();
    for _ in 0..2400 {
        assert_eq!(loud.next_frame(), quiet.next_frame());
    }
    // Direct Voice velocity zero is a valid modulation value, not note-off.
    assert!(!quiet.is_silent());
    for source in [3, 4] {
        let mut routed = Voice::new(48000.0, 22).unwrap();
        let mut dry = Voice::new(48000.0, 22).unwrap();
        dry.set_params(params).unwrap();
        let mut modulated = params;
        modulated.routes[source][34] = 1.0;
        routed.set_params(modulated).unwrap();
        let frequency = 440.0 * 2.0_f64.powf((84.0 - 69.0) / 12.0);
        routed.note_on(frequency, 127).unwrap();
        dry.note_on(frequency, 127).unwrap();
        let mut wet_energy = 0.0_f64;
        let mut dry_energy = 0.0_f64;
        for frame in 0..9600 {
            let wet = routed.next_frame();
            let dry = dry.next_frame();
            if frame >= 2400 {
                wet_energy += (wet[0] as f64).powi(2);
                dry_energy += (dry[0] as f64).powi(2);
            }
        }
        assert!(wet_energy > dry_energy * 4.0, "source {source}");
        assert_eq!(routed.params(), &modulated);
    }
}

#[test]
fn note_sources_apply_at_trigger_and_survive_release_until_retrigger() {
    let mut voice = Voice::new(48000.0, 23).unwrap();
    let mut params = VoiceParams::default();
    params.routes[3][2] = 0.5; // Phase must be ready before oscillator trigger.
    params.routes[4][34] = 0.25;
    params.globals[6] = 1000.0;
    voice.set_params(params).unwrap();
    for (frequency, velocity, key) in [
        (440.0 * 2.0_f64.powf(-9.0 / 12.0), 127, 0.0),
        (440.0 * 2.0_f64.powf(-21.0 / 12.0), 32, -0.2),
        (0.0, 0, -1.0),
        (f64::MIN_POSITIVE, 1, -1.0),
        (f64::MAX, 127, 1.0),
    ] {
        voice.note_on(frequency, velocity).unwrap();
        let t = voice.telemetry();
        assert_eq!(t.velocity, velocity as f32 / 127.0);
        assert!((t.key_track - key).abs() < 1e-6);
        assert!((t.effective[2] - t.velocity * 0.5).abs() < 1e-6);
        assert!((t.effective[34] - (params.normalized()[34] + key * 0.25)).abs() < 1e-6);
        for _ in 0..1000 {
            voice.next_frame();
        }
        voice.note_off();
        for _ in 0..100 {
            voice.next_frame();
        }
        assert_eq!(voice.telemetry().velocity, t.velocity);
        assert_eq!(voice.telemetry().key_track, t.key_track);
    }
}

#[test]
fn invalid_note_sources_preserve_running_audio_and_telemetry() {
    let mut reference = Voice::new(48000.0, 24).unwrap();
    let mut rejected = Voice::new(48000.0, 24).unwrap();
    let mut params = VoiceParams::default();
    params.routes[3][34] = -0.4;
    params.routes[4][2] = 0.5;
    params.oscillators[0].phase_random = 0.8;
    for voice in [&mut reference, &mut rejected] {
        voice.set_params(params).unwrap();
        voice.note_on(440.0, 32).unwrap();
        for _ in 0..1000 {
            voice.next_frame();
        }
    }
    let before = rejected.telemetry();
    for (frequency, velocity) in [
        (220.0, 128),
        (220.0, 255),
        (f64::NAN, 127),
        (-1.0, 0),
        (f64::INFINITY, 1),
    ] {
        assert!(rejected.note_on(frequency, velocity).is_err());
        let after = rejected.telemetry();
        assert_eq!(after.velocity, before.velocity);
        assert_eq!(after.key_track, before.key_track);
        assert_eq!(after.effective, before.effective);
    }
    for _ in 0..2000 {
        assert_eq!(reference.next_frame(), rejected.next_frame());
    }
}

#[test]
fn note_source_phase_routes_are_sampled_by_the_same_trigger() {
    let frequency = 440.0 * 2.0_f64.powf((84.0 - 69.0) / 12.0);
    for source in [3, 4] {
        let mut routed = Voice::new(48000.0, 25).unwrap();
        let mut reference = Voice::new(48000.0, 25).unwrap();
        let mut params = VoiceParams::default();
        params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.1]);
        params.routes[source][2] = 0.5;
        routed.set_params(params).unwrap();
        // First trigger has a different source value, so stale sampling fails.
        routed.note_on(220.0, 0).unwrap();
        routed.note_on(frequency, 127).unwrap();
        params.routes[source][2] = 0.0;
        params.oscillators[0].phase = if source == 3 { 0.5 } else { 0.2 };
        reference.set_params(params).unwrap();
        reference.note_on(frequency, 127).unwrap();
        for _ in 0..1000 {
            let actual = routed.next_frame();
            let expected = reference.next_frame();
            for channel in 0..2 {
                assert!((actual[channel] - expected[channel]).abs() < 1e-6);
            }
        }
    }
}

#[test]
fn filter_mode_selects_svf_taps() {
    assert_eq!(VoiceParams::default().filter_mode, FilterMode::Lowpass);

    fn render(mode: FilterMode, resonance: f32) -> Vec<[f32; 2]> {
        let mut voice = Voice::new(48000.0, 11).unwrap();
        let mut p = VoiceParams::default();
        p.oscillators[0].waveform = Waveform::Saw;
        p.globals[0] = 0.001;
        p.globals[2] = 1.0;
        p.globals[6] = 300.0;
        p.globals[7] = resonance;
        p.filter_mode = mode;
        voice.set_params(p).unwrap();
        voice.note_on(261.63, 127).unwrap();
        for _ in 0..4800 {
            voice.next_frame();
        }
        (0..4800).map(|_| voice.next_frame()).collect()
    }

    fn rms(frames: &[[f32; 2]]) -> f64 {
        let e: f64 = frames.iter().map(|f| (f[0] as f64).powi(2)).sum();
        (e / frames.len() as f64).sqrt()
    }

    fn bass(frames: &[[f32; 2]]) -> f64 {
        let mut s = 0.0;
        let mut e = 0.0;
        let a = 1.0 - (-std::f64::consts::TAU * 120.0 / 48000.0).exp();
        for f in frames {
            s += a * (f[0] as f64 - s);
            e += s * s;
        }
        e / frames.len() as f64
    }

    fn peak(frames: &[[f32; 2]]) -> f32 {
        frames.iter().map(|f| f[0].abs()).fold(0.0_f32, f32::max)
    }

    let lp = render(FilterMode::Lowpass, 0.1);
    let hp = render(FilterMode::Highpass, 0.1);
    let bp = render(FilterMode::Bandpass, 0.1);
    let bp_res = render(FilterMode::Bandpass, 0.8);
    assert!(bass(&hp) < bass(&lp));
    assert!(rms(&bp) < rms(&lp));
    assert!(peak(&bp_res) > peak(&bp));
}

#[test]
fn default_glide_is_instant_and_legato_glide_slides_pitch() {
    assert_eq!(VoiceParams::default().glide, 0.0);
    assert!(!VoiceParams::default().legato);

    let mut voice = Voice::new(48000.0, 9).unwrap();
    let mut params = VoiceParams::default();
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 0.5, 0.2]);
    voice.set_params(params).unwrap();
    voice.note_on(220.0, 127).unwrap();
    for _ in 0..1000 {
        voice.next_frame();
    }
    voice.note_on(440.0, 127).unwrap();
    assert!((voice.frequency() - 440.0).abs() < 1e-9);

    params.legato = true;
    params.glide = 0.0;
    voice.set_params(params).unwrap();
    voice.note_on(220.0, 127).unwrap();
    for _ in 0..1000 {
        voice.next_frame();
    }
    let env = voice.telemetry().env;
    voice.note_on(440.0, 127).unwrap();
    assert_eq!(voice.telemetry().env, env);
    assert!((voice.frequency() - 440.0).abs() < 1e-9);

    voice.note_on(220.0, 127).unwrap();
    params.glide = 0.1;
    voice.set_params(params).unwrap();
    assert!((voice.frequency() - 220.0).abs() < 1e-6);
    let env = voice.telemetry().env;
    voice.note_on(440.0, 127).unwrap();
    assert_eq!(voice.telemetry().env, env);
    for _ in 0..2400 {
        voice.next_frame();
    }
    let hz = voice.frequency();
    assert!(hz > 230.0 && hz < 430.0, "intermediate pitch {hz}");
    assert!(
        (voice.telemetry().env - 0.5).abs() < 0.02,
        "AMP ENV retriggered to {}",
        voice.telemetry().env
    );
}

#[test]
fn invalid_glide_is_rejected() {
    let mut voice = Voice::new(48000.0, 9).unwrap();
    let mut params = VoiceParams::default();
    params.glide = -0.1;
    assert!(voice.set_params(params).is_err());
    params.glide = 2.1;
    assert!(voice.set_params(params).is_err());
    params.glide = f32::NAN;
    assert!(voice.set_params(params).is_err());
    params.glide = 2.0;
    voice.set_params(params).unwrap();
}

#[test]
fn oscillator_fm_from_silent_master_changes_audio() {
    let mut free = Voice::new(48_000.0, 4).unwrap();
    let mut fm = Voice::new(48_000.0, 4).unwrap();
    let mut params = VoiceParams::default();
    params.oscillators[0].waveform = Waveform::Sine;
    params.oscillators[0].level = 0.0;
    params.oscillators[1].waveform = Waveform::Sine;
    params.oscillators[1].level = 1.0;
    params.globals[6] = 18000.0;
    free.set_params(params).unwrap();
    params.fm[1] = 0.75;
    fm.set_params(params).unwrap();
    params.fm[1] = 1.1;
    assert!(fm.set_params(params).is_err());
    assert_eq!(fm.params().fm[1], 0.75);
    free.note_on(220.0, 127).unwrap();
    fm.note_on(220.0, 127).unwrap();
    let mut different = false;
    for _ in 0..2048 {
        let a = free.next_frame();
        let b = fm.next_frame();
        assert!(b.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        if a != b {
            different = true;
        }
    }
    assert!(different);
}

#[test]
fn oscillator_ring_from_silent_master_changes_audio() {
    let mut free = Voice::new(48_000.0, 4).unwrap();
    let mut ring = Voice::new(48_000.0, 4).unwrap();
    let mut params = VoiceParams::default();
    params.oscillators[0].waveform = Waveform::Sine;
    params.oscillators[0].level = 0.0;
    params.oscillators[1].waveform = Waveform::Sine;
    params.oscillators[1].level = 1.0;
    params.globals[6] = 18000.0;
    free.set_params(params).unwrap();
    params.ring[1] = 0.75;
    ring.set_params(params).unwrap();
    params.ring[1] = 1.1;
    assert!(ring.set_params(params).is_err());
    assert_eq!(ring.params().ring[1], 0.75);
    free.note_on(220.0, 127).unwrap();
    ring.note_on(220.0, 127).unwrap();
    let mut different = false;
    for _ in 0..2048 {
        let a = free.next_frame();
        let b = ring.next_frame();
        assert!(b.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        if a != b {
            different = true;
        }
    }
    assert!(different);
}

#[test]
fn zero_noise_matches_default_oscillator_audio() {
    let mut plain = Voice::new(48000.0, 17).unwrap();
    let mut zero = Voice::new(48000.0, 17).unwrap();
    let mut params = VoiceParams::default();
    params.oscillators[0].waveform = Waveform::Saw;
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    plain.set_params(params).unwrap();
    params.noise = 0.0;
    zero.set_params(params).unwrap();
    plain.note_on(220.0, 127).unwrap();
    zero.note_on(220.0, 127).unwrap();
    for _ in 0..2048 {
        assert_eq!(plain.next_frame(), zero.next_frame());
    }
}

#[test]
fn white_noise_mixer_is_silent_at_zero_and_audible_when_oscillators_are_muted() {
    let mut silent = Voice::new(48000.0, 13).unwrap();
    let mut noisy = Voice::new(48000.0, 13).unwrap();
    let mut params = VoiceParams::default();
    params.oscillators[0].level = 0.0;
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    params.globals[6] = 18000.0;
    silent.set_params(params).unwrap();
    params.noise = 0.8;
    noisy.set_params(params).unwrap();
    params.noise = 1.1;
    assert!(noisy.set_params(params).is_err());
    assert_eq!(noisy.params().noise, 0.8);
    params.noise = f32::NAN;
    assert!(noisy.set_params(params).is_err());
    silent.note_on(220.0, 127).unwrap();
    noisy.note_on(220.0, 127).unwrap();
    let mut silent_energy = 0.0;
    let mut noisy_energy = 0.0;
    let mut peak = 0.0_f32;
    for _ in 0..4800 {
        let a = silent.next_frame();
        let b = noisy.next_frame();
        assert!(b.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        silent_energy += (a[0] as f64).powi(2);
        noisy_energy += (b[0] as f64).powi(2);
        peak = peak.max(b[0].abs());
        assert_eq!(b[0], b[1]);
    }
    assert!(silent_energy < 1e-12);
    assert!(noisy_energy > 0.01);
    assert!(peak > 0.01);
}

#[test]
fn white_noise_is_deterministic_per_seed_and_reaches_the_filter() {
    fn render(seed: u64, mode: FilterMode) -> Vec<f32> {
        let mut voice = Voice::new(48000.0, seed).unwrap();
        let mut p = VoiceParams::default();
        p.oscillators[0].level = 0.0;
        p.noise = 1.0;
        p.filter_mode = mode;
        p.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
        p.globals[6] = 400.0;
        p.globals[7] = 0.1;
        voice.set_params(p).unwrap();
        voice.note_on(440.0, 127).unwrap();
        (0..4096).map(|_| voice.next_frame()[0]).collect()
    }
    let lp = render(7, FilterMode::Lowpass);
    assert_eq!(lp, render(7, FilterMode::Lowpass));
    assert_ne!(lp, render(8, FilterMode::Lowpass));
    let hp = render(7, FilterMode::Highpass);
    assert_ne!(lp, hp);
    let bass = |frames: &[f32]| {
        let mut s = 0.0;
        let mut e = 0.0;
        let a = 1.0 - (-std::f64::consts::TAU * 120.0 / 48000.0).exp();
        for &x in frames {
            s += a * (f64::from(x) - s);
            e += s * s;
        }
        e / frames.len() as f64
    };
    assert!(bass(&hp) < bass(&lp));
}

#[test]
fn unison_spread_widens_stereo_and_rejects_invalid() {
    let mut narrow = Voice::new(48_000.0, 4).unwrap();
    let mut wide = Voice::new(48_000.0, 4).unwrap();
    let mut params = VoiceParams::default();
    params.oscillators[0].waveform = Waveform::Saw;
    params.oscillators[0].unison = 4;
    params.oscillators[0].detune = 18.0;
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    params.globals[6] = 18000.0;
    narrow.set_params(params).unwrap();
    params.spread[0] = 1.0;
    wide.set_params(params).unwrap();
    params.spread[0] = 1.1;
    assert!(wide.set_params(params).is_err());
    assert_eq!(wide.params().spread[0], 1.0);
    narrow.note_on(220.0, 127).unwrap();
    wide.note_on(220.0, 127).unwrap();
    let mut different = false;
    let mut wide_stereo = false;
    let mut narrow_centered = true;
    for _ in 0..2048 {
        let a = narrow.next_frame();
        let b = wide.next_frame();
        assert!(b.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        if a != b {
            different = true;
        }
        if b[0] != b[1] {
            wide_stereo = true;
        }
        if a[0] != a[1] {
            narrow_centered = false;
        }
    }
    assert!(different);
    assert!(wide_stereo);
    assert!(narrow_centered);
}

#[test]
fn analog_amounts_are_modulation_targets_without_changing_bases() {
    assert_eq!(plasma_kernel::target_range(48).unwrap(), (0.0, 2.0, false));
    assert_eq!(plasma_kernel::target_range(49).unwrap(), (0.0, 1.0, false));
    assert_eq!(plasma_kernel::target_range(50).unwrap(), (0.0, 1.0, false));
    assert_eq!(plasma_kernel::target_range(51).unwrap(), (0.0, 1.0, false));
    assert_eq!(plasma_kernel::target_range(52).unwrap(), (0.0, 1.0, false));
    assert!(plasma_kernel::target_range(53).is_err());

    let mut params = VoiceParams::default();
    params.oscillators[0].level = 0.0;
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    params.globals[6] = 18000.0;
    let mut dry = Voice::new(48_000.0, 11).unwrap();
    let mut wet = Voice::new(48_000.0, 11).unwrap();
    dry.set_params(params).unwrap();
    params.routes[3][40] = 1.0;
    wet.set_params(params).unwrap();
    dry.note_on(220.0, 127).unwrap();
    wet.note_on(220.0, 127).unwrap();
    let mut dry_energy = 0.0;
    let mut wet_energy = 0.0;
    for _ in 0..4800 {
        let a = dry.next_frame();
        let b = wet.next_frame();
        assert!(b.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        dry_energy += (a[0] as f64).powi(2);
        wet_energy += (b[0] as f64).powi(2);
    }
    assert!(dry_energy < 1e-12);
    assert!(wet_energy > 0.01);
    assert_eq!(wet.params().noise, 0.0);
    assert!((wet.telemetry().effective[40] - 1.0).abs() < 1e-6);

    let mut silent = Voice::new(48_000.0, 11).unwrap();
    silent.set_params(params).unwrap();
    silent.note_on(220.0, 0).unwrap();
    let mut silent_energy = 0.0;
    for _ in 0..4800 {
        let frame = silent.next_frame();
        silent_energy += (frame[0] as f64).powi(2);
    }
    assert!(silent_energy < 1e-12);

    let mut fm_params = VoiceParams::default();
    fm_params.oscillators[0].waveform = Waveform::Sine;
    fm_params.oscillators[0].level = 0.0;
    fm_params.oscillators[1].waveform = Waveform::Sine;
    fm_params.oscillators[1].level = 1.0;
    fm_params.globals[6] = 18000.0;
    let mut free = Voice::new(48_000.0, 4).unwrap();
    let mut routed = Voice::new(48_000.0, 4).unwrap();
    free.set_params(fm_params).unwrap();
    fm_params.routes[3][44] = 1.0;
    routed.set_params(fm_params).unwrap();
    free.note_on(220.0, 127).unwrap();
    routed.note_on(220.0, 127).unwrap();
    let mut different = false;
    for _ in 0..2048 {
        if free.next_frame() != routed.next_frame() {
            different = true;
        }
    }
    assert!(different);
    assert_eq!(routed.params().fm[1], 0.0);
    assert!((routed.telemetry().effective[44] - 1.0).abs() < 1e-6);

    let mut ring_params = VoiceParams::default();
    ring_params.oscillators[0].waveform = Waveform::Sine;
    ring_params.oscillators[0].level = 0.0;
    ring_params.oscillators[1].waveform = Waveform::Sine;
    ring_params.oscillators[1].level = 1.0;
    ring_params.globals[6] = 18000.0;
    let mut ring_free = Voice::new(48_000.0, 4).unwrap();
    let mut ring_routed = Voice::new(48_000.0, 4).unwrap();
    ring_free.set_params(ring_params).unwrap();
    ring_params.routes[3][46] = 1.0;
    ring_routed.set_params(ring_params).unwrap();
    ring_free.note_on(220.0, 127).unwrap();
    ring_routed.note_on(220.0, 127).unwrap();
    different = false;
    for _ in 0..2048 {
        if ring_free.next_frame() != ring_routed.next_frame() {
            different = true;
        }
    }
    assert!(different);
    assert_eq!(ring_routed.params().ring[1], 0.0);

    let mut spread_params = VoiceParams::default();
    spread_params.oscillators[0].waveform = Waveform::Saw;
    spread_params.oscillators[0].unison = 4;
    spread_params.oscillators[0].detune = 18.0;
    spread_params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    spread_params.globals[6] = 18000.0;
    let mut narrow = Voice::new(48_000.0, 4).unwrap();
    let mut wide = Voice::new(48_000.0, 4).unwrap();
    narrow.set_params(spread_params).unwrap();
    spread_params.routes[3][41] = 1.0;
    wide.set_params(spread_params).unwrap();
    narrow.note_on(220.0, 127).unwrap();
    wide.note_on(220.0, 127).unwrap();
    let mut wide_stereo = false;
    let mut narrow_centered = true;
    different = false;
    for _ in 0..2048 {
        let a = narrow.next_frame();
        let b = wide.next_frame();
        if a != b {
            different = true;
        }
        if b[0] != b[1] {
            wide_stereo = true;
        }
        if a[0] != a[1] {
            narrow_centered = false;
        }
    }
    assert!(different);
    assert!(wide_stereo);
    assert!(narrow_centered);
    assert_eq!(wide.params().spread[0], 0.0);
}

#[test]
fn oscillator_zero_self_mod_is_a_modulation_target_without_changing_bases() {
    let mut params = VoiceParams::default();
    params.oscillators[0].waveform = Waveform::Sine;
    params.oscillators[0].level = 1.0;
    params.globals[6] = 18000.0;
    let mut free = Voice::new(48_000.0, 4).unwrap();
    let mut fm = Voice::new(48_000.0, 4).unwrap();
    free.set_params(params).unwrap();
    params.routes[3][49] = 1.0;
    fm.set_params(params).unwrap();
    free.note_on(220.0, 127).unwrap();
    fm.note_on(220.0, 127).unwrap();
    let mut different = false;
    for _ in 0..2048 {
        if free.next_frame() != fm.next_frame() {
            different = true;
        }
    }
    assert!(different);
    assert_eq!(fm.params().fm[0], 0.0);
    assert!((fm.telemetry().effective[49] - 1.0).abs() < 1e-6);

    let mut ring_params = VoiceParams::default();
    ring_params.oscillators[0].waveform = Waveform::Sine;
    ring_params.oscillators[0].level = 1.0;
    ring_params.globals[6] = 18000.0;
    let mut ring_free = Voice::new(48_000.0, 4).unwrap();
    let mut ring_routed = Voice::new(48_000.0, 4).unwrap();
    ring_free.set_params(ring_params).unwrap();
    ring_params.routes[3][50] = 1.0;
    ring_routed.set_params(ring_params).unwrap();
    ring_free.note_on(220.0, 127).unwrap();
    ring_routed.note_on(220.0, 127).unwrap();
    different = false;
    for _ in 0..2048 {
        if ring_free.next_frame() != ring_routed.next_frame() {
            different = true;
        }
    }
    assert!(different);
    assert_eq!(ring_routed.params().ring[0], 0.0);
    assert!((ring_routed.telemetry().effective[50] - 1.0).abs() < 1e-6);
}

#[test]
fn glide_is_a_modulation_target_without_changing_base() {
    let mut params = VoiceParams::default();
    params.legato = true;
    params.glide = 0.0;
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 0.5, 0.2]);
    params.routes[3][48] = 1.0;

    let mut sliding = Voice::new(48_000.0, 9).unwrap();
    sliding.set_params(params).unwrap();
    sliding.note_on(220.0, 127).unwrap();
    for _ in 0..1000 {
        sliding.next_frame();
    }
    let env = sliding.telemetry().env;
    sliding.note_on(440.0, 127).unwrap();
    assert_eq!(sliding.telemetry().env, env);
    assert_eq!(sliding.params().glide, 0.0);
    assert!((sliding.telemetry().effective[48] - 1.0).abs() < 1e-6);
    for _ in 0..4800 {
        sliding.next_frame();
    }
    let hz = sliding.frequency();
    assert!(hz > 221.0 && hz < 250.0, "velocity-scaled glide pitch {hz}");

    let mut instant = Voice::new(48_000.0, 9).unwrap();
    instant.set_params(params).unwrap();
    instant.note_on(220.0, 0).unwrap();
    for _ in 0..1000 {
        instant.next_frame();
    }
    instant.note_on(440.0, 0).unwrap();
    assert!((instant.frequency() - 440.0).abs() < 1e-9);
    assert!(instant.telemetry().effective[48].abs() < 1e-6);
}

#[test]
fn modulated_zero_snaps_remainder_of_in_progress_glide() {
    let mut params = VoiceParams::default();
    params.legato = true;
    params.glide = 0.0;
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 0.5, 0.2]);
    params.routes[3][48] = 1.0;

    let mut voice = Voice::new(48_000.0, 9).unwrap();
    voice.set_params(params).unwrap();
    voice.note_on(220.0, 127).unwrap();
    for _ in 0..1000 {
        voice.next_frame();
    }
    voice.note_on(440.0, 127).unwrap();
    for _ in 0..4800 {
        voice.next_frame();
    }
    let mid = voice.frequency();
    assert!(mid > 221.0 && mid < 250.0, "pre-snap pitch {mid}");

    params.routes[3][48] = 0.0;
    voice.set_params(params).unwrap();
    for _ in 0..200 {
        voice.next_frame();
    }
    assert_eq!(voice.params().glide, 0.0);
    assert!(voice.telemetry().effective[48].abs() < 1e-6);
    assert!(
        (voice.frequency() - 440.0).abs() < 1e-6,
        "modulated zero should snap, got {}",
        voice.frequency()
    );
}

#[test]
fn in_progress_glide_keeps_trigger_duration_when_effective_stays_nonzero() {
    let mut params = VoiceParams::default();
    params.legato = true;
    params.glide = 0.05;
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 0.5, 0.2]);

    let mut voice = Voice::new(48_000.0, 9).unwrap();
    voice.set_params(params).unwrap();
    voice.note_on(220.0, 127).unwrap();
    for _ in 0..1000 {
        voice.next_frame();
    }
    voice.note_on(440.0, 127).unwrap();
    for _ in 0..1000 {
        voice.next_frame();
    }
    let mid = voice.frequency();
    assert!(mid > 221.0 && mid < 430.0, "mid-slide pitch {mid}");

    params.glide = 2.0;
    voice.set_params(params).unwrap();
    assert_eq!(voice.params().glide, 2.0);
    for _ in 0..2400 {
        voice.next_frame();
    }
    assert!(
        (voice.frequency() - 440.0).abs() < 1e-3,
        "snapshotted 50 ms slide should finish, got {}",
        voice.frequency()
    );
}

#[test]
fn default_pitch_bend_is_neutral_and_range_is_two_semitones() {
    let params = VoiceParams::default();
    assert_eq!(params.pitch_bend, 0.0);
    assert_eq!(params.pitch_bend_range, 2.0);
}

#[test]
fn octave_pitch_bend_matches_unbent_octave_and_preserves_key_track() {
    let mut bent = Voice::new(48_000.0, 3).unwrap();
    let mut octave = Voice::new(48_000.0, 3).unwrap();
    let mut params = VoiceParams::default();
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    params.pitch_bend = 1.0;
    params.pitch_bend_range = 12.0;
    bent.set_params(params).unwrap();
    params.pitch_bend = 0.0;
    octave.set_params(params).unwrap();
    bent.note_on(220.0, 64).unwrap();
    octave.note_on(440.0, 64).unwrap();
    assert!((bent.frequency() - 440.0).abs() < 1e-9);
    assert!((bent.telemetry().key_track + 3.0 / 60.0).abs() < 1e-5);
    assert!((octave.telemetry().key_track - 9.0 / 60.0).abs() < 1e-5);
    let mut a = [[0.0; 2]; 256];
    let mut b = [[0.0; 2]; 256];
    bent.render(&mut a);
    octave.render(&mut b);
    assert_eq!(a, b);
}

#[test]
fn live_pitch_bend_retunes_without_retriggering() {
    let mut voice = Voice::new(48_000.0, 11).unwrap();
    let mut params = VoiceParams::default();
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    voice.set_params(params).unwrap();
    voice.note_on(440.0, 127).unwrap();
    for _ in 0..2000 {
        voice.next_frame();
    }
    let env = voice.telemetry().env;
    let mut before = [[0.0; 2]; 128];
    voice.render(&mut before);
    params.pitch_bend = -1.0;
    params.pitch_bend_range = 12.0;
    voice.set_params(params).unwrap();
    assert!((voice.frequency() - 220.0).abs() < 1e-9);
    assert_eq!(voice.telemetry().env, env);
    let mut after = [[0.0; 2]; 128];
    voice.render(&mut after);
    assert_ne!(before, after);
    assert!(after.iter().all(|f| f.iter().all(|s| s.is_finite())));
    assert!((voice.telemetry().env - env).abs() < 0.02);
}

#[test]
fn invalid_pitch_bend_is_rejected_atomically() {
    let mut voice = Voice::new(48_000.0, 9).unwrap();
    let mut params = VoiceParams::default();
    params.pitch_bend = 1.0;
    params.pitch_bend_range = 12.0;
    voice.set_params(params).unwrap();
    params.pitch_bend = 1.1;
    assert!(voice.set_params(params).is_err());
    params.pitch_bend = -1.1;
    assert!(voice.set_params(params).is_err());
    params.pitch_bend = f32::NAN;
    assert!(voice.set_params(params).is_err());
    params.pitch_bend = 1.0;
    params.pitch_bend_range = -0.1;
    assert!(voice.set_params(params).is_err());
    params.pitch_bend_range = 24.1;
    assert!(voice.set_params(params).is_err());
    params.pitch_bend_range = f32::NAN;
    assert!(voice.set_params(params).is_err());
    assert_eq!(voice.params().pitch_bend, 1.0);
    assert_eq!(voice.params().pitch_bend_range, 12.0);
}

#[test]
fn pitch_bend_scales_an_in_progress_glide() {
    let mut params = VoiceParams::default();
    params.legato = true;
    params.glide = 0.1;
    params.pitch_bend = 1.0;
    params.pitch_bend_range = 12.0;
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 0.5, 0.2]);
    let mut voice = Voice::new(48_000.0, 9).unwrap();
    voice.set_params(params).unwrap();
    voice.note_on(220.0, 127).unwrap();
    for _ in 0..1000 {
        voice.next_frame();
    }
    assert!((voice.frequency() - 440.0).abs() < 1e-6);
    voice.note_on(440.0, 127).unwrap();
    for _ in 0..2400 {
        voice.next_frame();
    }
    let hz = voice.frequency();
    assert!(hz > 460.0 && hz < 860.0, "bent intermediate pitch {hz}");
}

#[test]
fn oscillator_sync_is_a_modulation_target_without_changing_bases() {
    let mut params = VoiceParams::default();
    params.oscillators[0].waveform = Waveform::Saw;
    params.oscillators[0].level = 0.0;
    params.oscillators[1].waveform = Waveform::Saw;
    params.oscillators[1].pitch = 7.0;
    params.oscillators[1].level = 1.0;
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    params.globals[6] = 18000.0;

    let mut free = Voice::new(48_000.0, 4).unwrap();
    let mut routed = Voice::new(48_000.0, 4).unwrap();
    let mut latched = Voice::new(48_000.0, 4).unwrap();
    free.set_params(params).unwrap();
    let mut routed_params = params;
    routed_params.routes[3][51] = 1.0;
    routed.set_params(routed_params).unwrap();
    params.sync[1] = true;
    latched.set_params(params).unwrap();
    free.note_on(220.0, 127).unwrap();
    routed.note_on(220.0, 127).unwrap();
    latched.note_on(220.0, 127).unwrap();
    let mut different = false;
    for _ in 0..2048 {
        let a = free.next_frame();
        let b = routed.next_frame();
        let c = latched.next_frame();
        assert_eq!(b, c);
        if a != b {
            different = true;
        }
    }
    assert!(different);
    assert!(!routed.params().sync[1]);
    assert!((routed.telemetry().effective[51] - 1.0).abs() < 1e-6);

    let mut below = Voice::new(48_000.0, 4).unwrap();
    routed_params.routes[3][51] = 0.4;
    below.set_params(routed_params).unwrap();
    below.note_on(220.0, 127).unwrap();
    let mut free = Voice::new(48_000.0, 4).unwrap();
    params.sync[1] = false;
    free.set_params(params).unwrap();
    free.note_on(220.0, 127).unwrap();
    for _ in 0..512 {
        assert_eq!(below.next_frame(), free.next_frame());
    }
    assert_eq!(below.telemetry().effective[51], 0.0);
    assert!(!below.params().sync[1]);
}

#[test]
fn default_mod_wheel_is_zero() {
    assert_eq!(VoiceParams::default().mod_wheel, 0.0);
}

#[test]
fn mod_wheel_is_a_unipolar_source_without_changing_bases() {
    let mut params = VoiceParams::default();
    params.oscillators[0].level = 0.0;
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    params.globals[6] = 18000.0;
    let mut dry = Voice::new(48_000.0, 11).unwrap();
    let mut wet = Voice::new(48_000.0, 11).unwrap();
    dry.set_params(params).unwrap();
    params.mod_wheel = 1.0;
    params.routes[5][40] = 1.0;
    wet.set_params(params).unwrap();
    dry.note_on(220.0, 127).unwrap();
    wet.note_on(220.0, 127).unwrap();
    let mut dry_energy = 0.0;
    let mut wet_energy = 0.0;
    for _ in 0..4800 {
        let a = dry.next_frame();
        let b = wet.next_frame();
        assert!(b.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        dry_energy += (a[0] as f64).powi(2);
        wet_energy += (b[0] as f64).powi(2);
    }
    assert!(dry_energy < 1e-12);
    assert!(wet_energy > 0.01);
    assert_eq!(wet.params().noise, 0.0);
    assert_eq!(wet.telemetry().mod_wheel, 1.0);
    assert!((wet.telemetry().effective[40] - 1.0).abs() < 1e-6);
    assert_eq!(wet.telemetry().velocity, 1.0);
    assert!((wet.telemetry().key_track + 3.0 / 60.0).abs() < 1e-5);
}

#[test]
fn live_mod_wheel_updates_without_retriggering() {
    let mut voice = Voice::new(48_000.0, 11).unwrap();
    let mut params = VoiceParams::default();
    params.globals[..4].copy_from_slice(&[0.001, 0.001, 1.0, 0.2]);
    params.routes[5][0] = 1.0;
    voice.set_params(params).unwrap();
    voice.note_on(440.0, 64).unwrap();
    for _ in 0..2000 {
        voice.next_frame();
    }
    let env = voice.telemetry().env;
    let velocity = voice.telemetry().velocity;
    let key = voice.telemetry().key_track;
    let mut before = [[0.0; 2]; 128];
    voice.render(&mut before);
    params.mod_wheel = 1.0;
    voice.set_params(params).unwrap();
    assert_eq!(voice.telemetry().mod_wheel, 1.0);
    assert_eq!(voice.telemetry().env, env);
    assert_eq!(voice.telemetry().velocity, velocity);
    assert_eq!(voice.telemetry().key_track, key);
    let mut after = [[0.0; 2]; 128];
    voice.render(&mut after);
    assert_ne!(before, after);
    assert!(after.iter().all(|f| f.iter().all(|s| s.is_finite())));
    assert!((voice.telemetry().env - env).abs() < 0.02);
    assert_eq!(voice.telemetry().velocity, velocity);
    assert_eq!(voice.telemetry().key_track, key);
}

#[test]
fn invalid_mod_wheel_is_rejected_atomically() {
    let mut voice = Voice::new(48_000.0, 9).unwrap();
    let mut params = VoiceParams::default();
    params.mod_wheel = 0.5;
    voice.set_params(params).unwrap();
    params.mod_wheel = 1.1;
    assert!(voice.set_params(params).is_err());
    params.mod_wheel = -0.1;
    assert!(voice.set_params(params).is_err());
    params.mod_wheel = f32::NAN;
    assert!(voice.set_params(params).is_err());
    assert_eq!(voice.params().mod_wheel, 0.5);
    assert_eq!(voice.telemetry().mod_wheel, 0.5);
}
