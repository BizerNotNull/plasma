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
