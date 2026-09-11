//! Run with `cargo run --manifest-path src/kernel/Cargo.toml --example voice`.
//! Optionally pass a WAV path to retain the rendered modulation/release demo.
use plasma_kernel::{Voice, VoiceParams, Waveform};
use std::io::Write;
fn render(
    cutoff: f32,
    modulated: bool,
) -> Result<(Vec<[f32; 2]>, f64), Box<dyn std::error::Error>> {
    let mut voice = Voice::new(48000.0, 42)?;
    let mut p = VoiceParams::default();
    p.oscillators[0].waveform = Waveform::Saw;
    p.oscillators[0].unison = 4;
    p.oscillators[0].detune = 12.0;
    p.globals[6] = cutoff;
    p.globals[7] = 0.4;
    if modulated {
        p.routes[0][34] = 0.2;
        p.routes[1][34] = 0.35;
        p.routes[1][7] = 0.6;
    }
    voice.set_params(p)?;
    voice.note_on(220.0)?;
    let mut frames = Vec::with_capacity(144000);
    let mut energy = 0.0;
    for i in 0..144000 {
        if i == 96000 {
            voice.note_off();
        }
        let frame = voice.next_frame();
        assert!(frame.iter().all(|v| v.is_finite()));
        if (24000..96000).contains(&i) {
            energy += frame.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
        }
        frames.push(frame);
    }
    assert!(voice.is_silent());
    assert_eq!(frames.last(), Some(&[0.0; 2]));
    Ok((frames, (energy / 144000.0).sqrt()))
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (_, bright) = render(18000.0, false)?;
    let (_, dark) = render(80.0, false)?;
    assert!(
        dark < bright * 0.25,
        "Cutoff must audibly attenuate the oscillator: {dark} vs {bright}"
    );
    let (baseline, _) = render(1200.0, false)?;
    let (frames, moving) = render(1200.0, true)?;
    let difference = frames
        .iter()
        .zip(&baseline)
        .flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| f64::from(a - b).powi(2)))
        .sum::<f64>();
    assert!(
        difference > 0.1,
        "Routes must change the rendered audio at the same base cutoff"
    );
    println!(
        "Voice smoke: bright RMS={bright:.6}, low-cutoff RMS={dark:.6}, ENV/LFO-routed RMS={moving:.6}, routed difference energy={difference:.6}; release reached exact silence."
    );
    if let Some(path) = std::env::args_os().nth(1) {
        let mut wav = std::fs::File::create(path)?;
        let bytes = (frames.len() * 4) as u32;
        wav.write_all(b"RIFF")?;
        wav.write_all(&(36 + bytes).to_le_bytes())?;
        wav.write_all(b"WAVEfmt ")?;
        wav.write_all(&16u32.to_le_bytes())?;
        wav.write_all(&1u16.to_le_bytes())?;
        wav.write_all(&2u16.to_le_bytes())?;
        wav.write_all(&48000u32.to_le_bytes())?;
        wav.write_all(&192000u32.to_le_bytes())?;
        wav.write_all(&4u16.to_le_bytes())?;
        wav.write_all(&16u16.to_le_bytes())?;
        wav.write_all(b"data")?;
        wav.write_all(&bytes.to_le_bytes())?;
        for frame in frames {
            for value in frame {
                wav.write_all(&((value * 32767.0) as i16).to_le_bytes())?;
            }
        }
    }
    Ok(())
}
