use super::*;

#[test]
fn buffer_preserves_fifo_and_velocity_through_production_rendering() {
    let synth = Synth::new();
    let mut renderer = AudioRenderer::new(synth.clone(), 48_000.0, 19).unwrap();
    let mut reference = PolySynth::new(48_000.0, 19).unwrap();
    reference.set_params(synth.voice_params().unwrap()).unwrap();
    for (note, velocity) in [(60, 100), (64, 40), (60, 0), (60, 75), (67, 127), (64, 0)] {
        synth.note_on(note, velocity).unwrap();
        reference.note_on(note, velocity).unwrap();
    }
    let mut actual = [0.0_f32; 512];
    renderer.render_interleaved(&mut actual, 2);
    for frame in actual.chunks_exact(2) {
        assert_eq!(frame, reference.next_frame());
    }
    assert_eq!(
        renderer.active_voice_count(),
        reference.active_voice_count()
    );
    assert!(actual.iter().any(|sample| sample.abs() > 0.0001));
}

#[test]
fn note_on_and_off_in_one_buffer_does_not_leave_a_held_note() {
    let synth = Synth::new();
    let mut renderer = AudioRenderer::new(synth.clone(), 48_000.0, 7).unwrap();
    synth.note_on(60, 100).unwrap();
    synth.note_off(60).unwrap();
    let mut frames = [[0.0; 2]; 1024];
    renderer.render(&mut frames);
    assert_eq!(renderer.active_voice_count(), 0);
    synth.note_off(60).unwrap();
    synth.note_on(60, 100).unwrap();
    renderer.render(&mut frames);
    assert_eq!(renderer.active_voice_count(), 1);
    assert!(frames.iter().flatten().any(|sample| sample.abs() > 0.0001));
}

#[test]
fn overflow_releases_sounding_notes_discards_stale_events_and_recovers() {
    let synth = Synth::new();
    synth.set_global(3, 0.01).unwrap();
    let mut renderer = AudioRenderer::new(synth.clone(), 48_000.0, 3).unwrap();
    let mut frames = [[0.0; 2]; 2048];
    synth.note_on(60, 127).unwrap();
    renderer.render(&mut frames);
    assert_eq!(renderer.active_voice_count(), 1);
    for _ in 0..events::CAPACITY {
        synth.note_on(64, 100).unwrap();
    }
    assert!(synth.note_off(60).is_err());
    renderer.render(&mut frames);
    assert_eq!(renderer.active_voice_count(), 0);
    synth.note_on(67, 127).unwrap();
    renderer.render(&mut frames);
    assert_eq!(renderer.active_voice_count(), 1);
    assert!(frames.iter().flatten().any(|sample| sample.abs() > 0.0001));
}

#[test]
fn all_notes_off_succeeds_at_capacity_and_preserves_events_after_boundary() {
    let synth = Synth::new();
    let mut renderer = AudioRenderer::new(synth.clone(), 48_000.0, 5).unwrap();
    for _ in 0..events::CAPACITY {
        synth.note_on(60, 127).unwrap();
    }
    synth.all_notes_off().unwrap();
    let mut frames = [[0.0; 2]; 1024];
    renderer.render(&mut frames);
    assert_eq!(renderer.active_voice_count(), 0);
    synth.note_on(60, 127).unwrap();
    synth.all_notes_off().unwrap();
    synth.note_on(64, 127).unwrap();
    renderer.render(&mut frames);
    assert_eq!(renderer.active_voice_count(), 1);
    synth.note_off(60).unwrap();
    renderer.render(&mut frames);
    assert_eq!(renderer.active_voice_count(), 1);
    synth.set_global(3, 0.01).unwrap();
    synth.note_off(64).unwrap();
    renderer.render(&mut frames);
    assert_eq!(renderer.active_voice_count(), 0);
}

#[test]
fn consumers_are_exclusive_and_failed_creation_releases_claim() {
    let synth = Synth::new();
    assert!(AudioRenderer::new(synth.clone(), 0.0, 1).is_err());
    let renderer = AudioRenderer::new(synth.clone(), 48_000.0, 1).unwrap();
    assert!(AudioRenderer::new(synth.clone(), 48_000.0, 1).is_err());
    // Claim rejection precedes device discovery, so this needs no audio hardware.
    assert!(AudioOutput::start(synth.clone()).is_err());
    drop(renderer);
    let mut replacement = AudioRenderer::new(synth.clone(), 48_000.0, 1).unwrap();
    synth.note_on(60, 127).unwrap();
    replacement.render(&mut [[0.0; 2]; 64]);
    assert_eq!(replacement.active_voice_count(), 1);
}

#[test]
fn clone_writers_preserve_each_notes_order_and_validate_midi() {
    let synth = Synth::new();
    let mut renderer = AudioRenderer::new(synth.clone(), 48_000.0, 9).unwrap();
    let writers: Vec<_> = (60..68)
        .map(|note| {
            let writer = synth.clone();
            std::thread::spawn(move || {
                writer.note_on(note, 127).unwrap();
                writer.note_off(note).unwrap();
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    assert!(synth.note_on(128, 100).is_err());
    assert!(synth.note_on(60, 128).is_err());
    assert!(synth.note_off(255).is_err());
    renderer.render(&mut [[0.0; 2]; 1024]);
    assert_eq!(renderer.active_voice_count(), 0);
}

#[test]
fn mod_envelope_shapes_filter_independently_through_audio_renderer() {
    let synth = Synth::new();
    let baseline = Synth::new();
    for controls in [&synth, &baseline] {
        controls.set_global(0, 0.001).unwrap();
        controls.set_global(1, 0.001).unwrap();
        controls.set_global(2, 1.0).unwrap();
        controls.set_global(3, 0.01).unwrap();
        controls.set_global(6, 100.0).unwrap();
        controls.set_global(8, 0.001).unwrap();
        controls.set_global(9, 0.02).unwrap();
        controls.set_global(10, 0.0).unwrap();
        controls.set_global(11, 10.0).unwrap();
    }
    synth.set_route(34, 2, 0.8).unwrap();
    let mut renderer = AudioRenderer::new(synth.clone(), 48_000.0, 42).unwrap();
    let mut reference = AudioRenderer::new(baseline.clone(), 48_000.0, 42).unwrap();
    // Settle the initial cutoff smoothing before measuring the note transient.
    renderer.render(&mut [[0.0; 2]; 4096]);
    reference.render(&mut [[0.0; 2]; 4096]);
    synth.note_on(69, 127).unwrap();
    baseline.note_on(69, 127).unwrap();
    let mut actual = [[0.0; 2]; 256];
    let mut dry = actual;
    renderer.render(&mut actual);
    reference.render(&mut dry);
    let energy = |frames: &[[f32; 2]]| {
        frames
            .iter()
            .flatten()
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>()
    };
    assert!(energy(&actual) > energy(&dry) * 2.0);
    let attack = synth.telemetry();
    assert!(attack.mod_env > 0.5);
    assert_eq!(attack.env, baseline.telemetry().env);
    for _ in 0..20 {
        renderer.render(&mut actual);
    }
    let held = synth.telemetry();
    assert_eq!(held.env, 1.0);
    assert_eq!(held.mod_env, 0.0);
    assert!(held.effective[34] < attack.effective[34]);

    // A long, nonzero modulation tail must not keep a silent AMP voice alive.
    synth.set_global(10, 1.0).unwrap();
    for _ in 0..20 {
        renderer.render(&mut actual);
    }
    assert!(synth.telemetry().mod_env > 0.9);
    synth.note_off(69).unwrap();
    for _ in 0..4 {
        renderer.render(&mut actual);
    }
    assert_eq!(renderer.active_voice_count(), 0);
    assert!(actual.iter().flatten().all(|v| *v == 0.0));
}

#[test]
fn performance_routes_change_rendered_audio_and_can_be_removed_live() {
    for source in [3, 4] {
        let synth = Synth::new();
        let baseline = Synth::new();
        for controls in [&synth, &baseline] {
            controls.set_global(0, 0.001).unwrap();
            controls.set_global(1, 0.001).unwrap();
            controls.set_global(2, 1.0).unwrap();
            controls.set_global(6, 100.0).unwrap();
        }
        synth.set_route(34, source, 1.0).unwrap();
        let mut renderer = AudioRenderer::new(synth.clone(), 48_000.0, 42).unwrap();
        let mut reference = AudioRenderer::new(baseline.clone(), 48_000.0, 42).unwrap();
        synth.note_on(84, 100).unwrap();
        baseline.note_on(84, 100).unwrap();
        let mut actual = [[0.0; 2]; 4096];
        let mut dry = actual;
        renderer.render(&mut actual);
        reference.render(&mut dry);
        let energy = |frames: &[[f32; 2]]| {
            frames
                .iter()
                .flatten()
                .map(|v| f64::from(*v).powi(2))
                .sum::<f64>()
        };
        assert!(energy(&actual) > energy(&dry) * 4.0);
        let meters = synth.telemetry();
        assert!((meters.velocity - 100.0 / 127.0).abs() < 1e-6);
        assert!((meters.key_track - 0.4).abs() < 1e-6);
        synth.set_route(34, source, 0.0).unwrap();
        for _ in 0..4 {
            renderer.render(&mut actual);
            reference.render(&mut dry);
        }
        assert!((energy(&actual) / energy(&dry) - 1.0).abs() < 0.001);
    }
}

#[test]
fn set_filter_mode_is_readable_on_voice_params() {
    let synth = Synth::new();
    assert_eq!(
        synth.voice_params().unwrap().filter_mode,
        FilterMode::Lowpass
    );
    synth.set_filter_mode(FilterMode::Highpass).unwrap();
    assert_eq!(
        synth.voice_params().unwrap().filter_mode,
        FilterMode::Highpass
    );
    synth.set_filter_mode(FilterMode::Bandpass).unwrap();
    assert_eq!(
        synth.voice_params().unwrap().filter_mode,
        FilterMode::Bandpass
    );
}

#[test]
fn set_glide_and_legato_round_trip() {
    let synth = Synth::new();
    assert_eq!(synth.voice_params().unwrap().glide, 0.0);
    assert!(!synth.voice_params().unwrap().legato);
    synth.set_glide(0.25).unwrap();
    synth.set_legato(true).unwrap();
    let params = synth.voice_params().unwrap();
    assert_eq!(params.glide, 0.25);
    assert!(params.legato);
    assert!(synth.set_glide(-0.1).is_err());
    assert!(synth.set_glide(2.1).is_err());
    assert!(synth.set_glide(f32::NAN).is_err());
    assert_eq!(synth.voice_params().unwrap().glide, 0.25);
    assert!(synth.voice_params().unwrap().legato);
}

#[test]
fn oscillator_sync_round_trips_and_changes_rendered_audio() {
    let free = Synth::new();
    let synced = Synth::new();
    let slave = OscillatorParams {
        waveform: Waveform::Saw,
        pitch: 7.0,
        level: 1.0,
        ..Default::default()
    };
    free.set_params(1, slave).unwrap();
    synced.set_params(1, slave).unwrap();
    synced.set_sync(1, true).unwrap();
    assert!(synced.voice_params().unwrap().sync[1]);
    assert!(!free.voice_params().unwrap().sync[1]);
    assert!(!synced.voice_params().unwrap().sync[0]);
    assert!(synced.set_sync(3, true).is_err());

    free.note_on(60, 127).unwrap();
    synced.note_on(60, 127).unwrap();
    let mut free_r = AudioRenderer::new(free, 48_000.0, 3).unwrap();
    let mut sync_r = AudioRenderer::new(synced, 48_000.0, 3).unwrap();
    let mut a = [0.0_f32; 2048];
    let mut b = [0.0_f32; 2048];
    free_r.render_interleaved(&mut a, 2);
    sync_r.render_interleaved(&mut b, 2);
    assert_ne!(a, b);
    assert!(b.iter().all(|s| s.is_finite()));
}

#[test]
fn oscillator_fm_round_trips_and_changes_rendered_audio() {
    let free = Synth::new();
    let fm = Synth::new();
    let master = OscillatorParams {
        waveform: Waveform::Sine,
        level: 0.0,
        ..Default::default()
    };
    let slave = OscillatorParams {
        waveform: Waveform::Sine,
        pitch: 19.0,
        level: 1.0,
        ..Default::default()
    };
    free.set_params(0, master).unwrap();
    free.set_params(1, slave).unwrap();
    fm.set_params(0, master).unwrap();
    fm.set_params(1, slave).unwrap();
    fm.set_fm(1, 0.8).unwrap();
    assert_eq!(fm.voice_params().unwrap().fm[1], 0.8);
    assert_eq!(free.voice_params().unwrap().fm[1], 0.0);
    assert_eq!(fm.voice_params().unwrap().fm[0], 0.0);
    assert!(fm.set_fm(3, 0.5).is_err());
    assert!(fm.set_fm(1, -0.1).is_err());
    assert!(fm.set_fm(1, 1.1).is_err());
    assert_eq!(fm.voice_params().unwrap().fm[1], 0.8);

    free.note_on(60, 127).unwrap();
    fm.note_on(60, 127).unwrap();
    let mut free_r = AudioRenderer::new(free, 48_000.0, 5).unwrap();
    let mut fm_r = AudioRenderer::new(fm, 48_000.0, 5).unwrap();
    let mut a = [0.0_f32; 4096];
    let mut b = [0.0_f32; 4096];
    free_r.render_interleaved(&mut a, 2);
    fm_r.render_interleaved(&mut b, 2);
    assert_ne!(a, b);
    assert!(b.iter().all(|s| s.is_finite()));
}

#[test]
fn oscillator_ring_round_trips_and_changes_rendered_audio() {
    let free = Synth::new();
    let ring = Synth::new();
    let master = OscillatorParams {
        waveform: Waveform::Sine,
        level: 0.0,
        ..Default::default()
    };
    let slave = OscillatorParams {
        waveform: Waveform::Sine,
        pitch: 19.0,
        level: 1.0,
        ..Default::default()
    };
    free.set_params(0, master).unwrap();
    free.set_params(1, slave).unwrap();
    ring.set_params(0, master).unwrap();
    ring.set_params(1, slave).unwrap();
    ring.set_ring(1, 0.8).unwrap();
    assert_eq!(ring.voice_params().unwrap().ring[1], 0.8);
    assert_eq!(free.voice_params().unwrap().ring[1], 0.0);
    assert_eq!(ring.voice_params().unwrap().ring[0], 0.0);
    assert!(ring.set_ring(3, 0.5).is_err());
    assert!(ring.set_ring(1, -0.1).is_err());
    assert!(ring.set_ring(1, 1.1).is_err());
    assert_eq!(ring.voice_params().unwrap().ring[1], 0.8);

    free.note_on(60, 127).unwrap();
    ring.note_on(60, 127).unwrap();
    let mut free_r = AudioRenderer::new(free, 48_000.0, 5).unwrap();
    let mut ring_r = AudioRenderer::new(ring, 48_000.0, 5).unwrap();
    let mut a = [0.0_f32; 4096];
    let mut b = [0.0_f32; 4096];
    free_r.render_interleaved(&mut a, 2);
    ring_r.render_interleaved(&mut b, 2);
    assert_ne!(a, b);
    assert!(b.iter().all(|s| s.is_finite()));
}
