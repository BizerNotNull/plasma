use plasma_kernel::{OscillatorBank, OscillatorParams, Waveform};

fn bank(params: OscillatorParams) -> OscillatorBank {
    let mut bank = OscillatorBank::new(48_000.0, 7).unwrap();
    bank.set_params(0, params).unwrap();
    bank.note_on(440.0).unwrap();
    bank
}

#[test]
fn detuned_voices_match_analytical_frequencies() {
    for unison in 1..=4 {
        let p = OscillatorParams {
            pitch: 12.0,
            fine: -37.0,
            phase: 0.2,
            unison,
            detune: 23.0,
            pan: -1.0,
            level: 0.4,
            ..Default::default()
        };
        let mut source = bank(p);
        for n in 0..1000 {
            let expected: f64 = (0..unison)
                .map(|voice| {
                    let position = if unison == 1 {
                        0.0
                    } else {
                        2.0 * f64::from(voice) / f64::from(unison - 1) - 1.0
                    };
                    let hz = 440.0 * 2.0_f64.powf((1200.0 - 37.0 + position * 23.0) / 1200.0);
                    (std::f64::consts::TAU * (0.2 + n as f64 * hz / 48_000.0)).sin()
                })
                .sum::<f64>()
                * 0.4
                / f64::from(unison);
            let frame = source.next_frame();
            assert!((f64::from(frame[0]) - expected).abs() < 1e-6);
            assert_eq!(frame[1], 0.0);
        }
    }
}

#[test]
fn rejected_controls_leave_audio_unchanged() {
    let mut source = bank(Default::default());
    let mut reference = bank(Default::default());
    for value in [f64::NAN, f64::INFINITY, -1.0, 1.1] {
        let invalid = OscillatorParams {
            phase_random: value,
            ..Default::default()
        };
        assert!(source.set_params(0, invalid).is_err());
    }
    for unison in [0, 5, 255] {
        assert!(
            source
                .set_params(
                    0,
                    OscillatorParams {
                        unison,
                        ..Default::default()
                    }
                )
                .is_err()
        );
    }
    assert!(source.set_params(3, Default::default()).is_err());
    assert!(source.note_on(f64::NAN).is_err());
    assert!(source.set_sample_rate(0.0).is_err());
    for _ in 0..128 {
        assert_eq!(source.next_frame(), reference.next_frame());
    }
}

#[test]
fn random_phase_is_repeatable_but_changes_between_triggers() {
    let params = OscillatorParams {
        phase_random: 1.0,
        unison: 4,
        ..Default::default()
    };
    let mut a = bank(params);
    let mut b = bank(params);
    let first = a.next_frame();
    assert_eq!(first, b.next_frame());
    a.note_on(440.0).unwrap();
    b.note_on(440.0).unwrap();
    let second = a.next_frame();
    assert_eq!(second, b.next_frame());
    assert_ne!(first, second);
    let mut fixed = bank(OscillatorParams {
        phase: 0.25,
        ..Default::default()
    });
    let first = fixed.next_frame();
    fixed.note_on(440.0).unwrap();
    assert_eq!(first, fixed.next_frame());
}

#[test]
fn render_chunk_boundaries_do_not_change_audio() {
    let params = OscillatorParams {
        waveform: Waveform::Saw,
        unison: 4,
        detune: 19.0,
        ..Default::default()
    };
    let mut a = bank(params);
    let mut b = bank(params);
    let mut full = [[0.0; 2]; 1024];
    let mut chunks = full;
    a.render(&mut full);
    for chunk in chunks.chunks_mut(73) {
        b.render(chunk);
    }
    assert_eq!(full, chunks);
}

#[test]
fn pulse_duty_and_nyquist_edges_remain_finite() {
    for waveform in [
        Waveform::Sine,
        Waveform::Triangle,
        Waveform::Saw,
        Waveform::Pulse,
    ] {
        let mut source = bank(OscillatorParams {
            waveform,
            pulse_width: 0.01,
            ..Default::default()
        });
        for hz in [0.0, 0.001, 23_999.0, 24_000.0, f64::MAX] {
            source.note_on(hz).unwrap();
            for _ in 0..128 {
                let frame = source.next_frame();
                assert!(frame.iter().all(|x| x.is_finite() && x.abs() <= 1.5));
                if hz == 0.0 || hz >= 24_000.0 {
                    assert_eq!(frame, [0.0; 2]);
                }
            }
        }
    }
    let mut source = bank(OscillatorParams {
        waveform: Waveform::Pulse,
        pulse_width: 0.25,
        pan: -1.0,
        ..Default::default()
    });
    source.note_on(480.0).unwrap();
    let mean = (0..1000)
        .map(|_| source.next_frame()[0] as f64)
        .sum::<f64>()
        / 1000.0;
    assert!((mean + 0.5).abs() < 1e-6);
}

fn period_error(frames: &[f32], period: usize) -> f64 {
    frames[period..]
        .iter()
        .zip(&frames[..frames.len() - period])
        .map(|(a, b)| (a - b).abs() as f64)
        .sum::<f64>()
        / (frames.len() - period) as f64
}

#[test]
fn hard_sync_locks_slave_to_master_period() {
    fn render(sync: bool) -> Vec<f32> {
        let mut bank = OscillatorBank::new(48_000.0, 1).unwrap();
        bank.set_params(
            0,
            OscillatorParams {
                waveform: Waveform::Saw,
                level: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        bank.set_params(
            1,
            OscillatorParams {
                waveform: Waveform::Saw,
                pitch: 7.0,
                level: 1.0,
                pan: -1.0,
                ..Default::default()
            },
        )
        .unwrap();
        bank.set_sync(1, sync).unwrap();
        bank.set_params(
            2,
            OscillatorParams {
                level: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        bank.note_on(200.0).unwrap();
        (0..4800).map(|_| bank.next_frame()[0]).collect()
    }

    let free = render(false);
    let synced = render(true);
    assert_ne!(free, synced);
    assert!(synced.iter().all(|s| s.is_finite() && s.abs() <= 1.5));
    let sync_err = period_error(&synced[480..], 240);
    let free_err = period_error(&free[480..], 240);
    assert!(
        sync_err < free_err * 0.05,
        "sync period error {sync_err} vs free {free_err}"
    );
}

#[test]
fn oscillator_zero_sync_flag_does_not_change_audio() {
    let params = OscillatorParams {
        waveform: Waveform::Saw,
        ..Default::default()
    };
    let mut synced = bank(params);
    synced.set_sync(0, true).unwrap();
    let mut free = bank(params);
    for _ in 0..512 {
        assert_eq!(synced.next_frame(), free.next_frame());
    }
}

fn fm_bank(modulator_level: f64, fm: f64) -> OscillatorBank {
    let mut bank = OscillatorBank::new(48_000.0, 3).unwrap();
    bank.set_params(
        0,
        OscillatorParams {
            waveform: Waveform::Sine,
            level: modulator_level,
            ..Default::default()
        },
    )
    .unwrap();
    bank.set_params(
        1,
        OscillatorParams {
            waveform: Waveform::Sine,
            pitch: 19.0,
            level: 1.0,
            pan: -1.0,
            ..Default::default()
        },
    )
    .unwrap();
    bank.set_params(
        2,
        OscillatorParams {
            level: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    bank.set_fm(1, fm).unwrap();
    bank.note_on(110.0).unwrap();
    bank
}

#[test]
fn zero_fm_matches_unmodulated_audio() {
    let mut free = OscillatorBank::new(48_000.0, 3).unwrap();
    free.set_params(
        0,
        OscillatorParams {
            waveform: Waveform::Sine,
            level: 1.0,
            ..Default::default()
        },
    )
    .unwrap();
    free.set_params(
        1,
        OscillatorParams {
            waveform: Waveform::Sine,
            pitch: 19.0,
            level: 1.0,
            pan: -1.0,
            ..Default::default()
        },
    )
    .unwrap();
    free.set_params(
        2,
        OscillatorParams {
            level: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    free.note_on(110.0).unwrap();
    let mut zero = fm_bank(1.0, 0.0);
    for _ in 0..1024 {
        assert_eq!(free.next_frame(), zero.next_frame());
    }
}

#[test]
fn oscillator_zero_fm_does_not_change_audio() {
    let mut carrier = fm_bank(0.0, 0.0);
    let mut ignored = fm_bank(0.0, 0.0);
    ignored.set_fm(0, 1.0).unwrap();
    for _ in 0..1024 {
        assert_eq!(carrier.next_frame(), ignored.next_frame());
    }
}

#[test]
fn silent_modulator_still_frequency_modulates() {
    let mut free = fm_bank(0.0, 0.0);
    let mut fm = fm_bank(0.0, 1.0);
    let mut different = false;
    for _ in 0..2048 {
        let a = free.next_frame();
        let b = fm.next_frame();
        assert!(b.iter().all(|s| s.is_finite()));
        if a != b {
            different = true;
        }
    }
    assert!(different, "silent OSC 1 must still FM OSC 2");
}

#[test]
fn fm_changes_audio_and_rejects_invalid_amounts() {
    let mut source = fm_bank(1.0, 0.5);
    let mut reference = fm_bank(1.0, 0.5);
    for amount in [f64::NAN, f64::INFINITY, -0.01, 1.01] {
        assert!(source.set_fm(1, amount).is_err());
    }
    assert!(source.set_fm(3, 0.5).is_err());
    for _ in 0..256 {
        assert_eq!(source.next_frame(), reference.next_frame());
    }
    let mut free = fm_bank(1.0, 0.0);
    let mut modulated = fm_bank(1.0, 0.5);
    let mut different = false;
    for _ in 0..2048 {
        if free.next_frame() != modulated.next_frame() {
            different = true;
        }
    }
    assert!(different);
}

#[test]
fn through_zero_fm_reverses_carrier_phase() {
    fn carrier(fm: f64, modulator_phase: f64) -> OscillatorBank {
        let mut bank = OscillatorBank::new(48_000.0, 3).unwrap();
        bank.set_params(
            0,
            OscillatorParams {
                waveform: Waveform::Sine,
                phase: modulator_phase,
                level: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        bank.set_params(
            1,
            OscillatorParams {
                waveform: Waveform::Sine,
                pitch: 36.0,
                level: 1.0,
                pan: -1.0,
                ..Default::default()
            },
        )
        .unwrap();
        bank.set_params(
            2,
            OscillatorParams {
                level: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        bank.set_fm(1, fm).unwrap();
        bank.note_on(1.0).unwrap();
        bank
    }
    let mut free = carrier(0.0, 0.75);
    let mut reversed = carrier(1.0, 0.75);
    let mut same = carrier(1.0, 0.25);
    assert_eq!(free.next_frame()[0], 0.0);
    assert_eq!(reversed.next_frame()[0], 0.0);
    assert_eq!(same.next_frame()[0], 0.0);
    let free_s = free.next_frame()[0];
    let reversed_s = reversed.next_frame()[0];
    let same_s = same.next_frame()[0];
    assert!(free_s > 0.0, "unmodulated sine must start forward, got {free_s}");
    assert!(
        reversed_s < 0.0,
        "modulator at -1 must reverse phase, got {reversed_s}"
    );
    assert!(same_s > 0.0, "modulator at +1 must stay forward, got {same_s}");
}

#[test]
fn oscillator_two_takes_fm_from_oscillator_zero() {
    let mut free = OscillatorBank::new(48_000.0, 3).unwrap();
    let mut fm = OscillatorBank::new(48_000.0, 3).unwrap();
    for bank in [&mut free, &mut fm] {
        bank.set_params(
            0,
            OscillatorParams {
                waveform: Waveform::Sine,
                level: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        bank.set_params(
            1,
            OscillatorParams {
                level: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        bank.set_params(
            2,
            OscillatorParams {
                waveform: Waveform::Sine,
                pitch: 19.0,
                level: 1.0,
                pan: -1.0,
                ..Default::default()
            },
        )
        .unwrap();
    }
    fm.set_fm(2, 1.0).unwrap();
    free.note_on(110.0).unwrap();
    fm.note_on(110.0).unwrap();
    let mut different = false;
    for _ in 0..2048 {
        if free.next_frame() != fm.next_frame() {
            different = true;
        }
    }
    assert!(different, "OSC 3 must accept FM from OSC 1");
}
