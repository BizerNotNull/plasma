mod patch;

use patch::{Control, Patch, Request, control_name};
use plasma_kernel::{TARGET_COUNT, Voice, VoiceParams, target_range};
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;

const MAX_REQUEST_BYTES: u64 = 65_536;
type Result<T, E = Box<dyn Error>> = std::result::Result<T, E>;

fn describe() -> Result<()> {
    let controls = (0..TARGET_COUNT)
        .map(|index| {
            let (min, max, log) = target_range(index)?;
            Ok(Control {
                name: control_name(index),
                min: min.to_string().parse()?,
                max: max.to_string().parse()?,
                log,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let description = serde_json::json!({
        "default_patch": Patch::from_params(&VoiceParams::default()),
        "controls": controls,
        "waveforms": ["sine", "triangle", "saw", "pulse"],
        "lfo_waves": ["sine", "triangle", "saw", "square"],
        "sources": ["amp_env", "lfo", "mod_env", "velocity", "key_track"],
        "velocity": "Required integer 0..127; modulation only, no automatic amplitude scaling.",
        "key_track": "clamp((69 + 12*log2(frequency/440) - 60)/60, -1, 1); MIDI 60 is zero.",
    });
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &description)?;
    writeln!(stdout)?;
    Ok(())
}

fn render(request_path: &Path, output_path: &Path) -> Result<()> {
    let file = File::open(request_path)
        .map_err(|error| format!("cannot open request {}: {error}", request_path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_REQUEST_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        return Err(format!("request exceeds {MAX_REQUEST_BYTES} bytes").into());
    }
    let request: Request =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid request JSON: {error}"))?;
    if !(8_000..=192_000).contains(&request.sample_rate) {
        return Err("sample_rate must be an integer in [8000, 192000] Hz".into());
    }
    if !request.frequency.is_finite()
        || request.frequency <= 0.0
        || request.frequency >= f64::from(request.sample_rate) / 2.0
    {
        return Err("frequency must be finite, positive and below sample_rate / 2".into());
    }
    if request.velocity > 127 {
        return Err("velocity must be an integer in 0..=127".into());
    }
    if !request.duration.is_finite()
        || !request.gate.is_finite()
        || request.gate <= 0.0
        || request.gate > request.duration
        || request.duration > 30.0
    {
        return Err(
            "duration and gate must be finite seconds satisfying 0 < gate <= duration <= 30".into(),
        );
    }
    let frame_count = (request.duration * f64::from(request.sample_rate)).round() as u64;
    let gate_frame = (request.gate * f64::from(request.sample_rate)).round() as u64;
    if frame_count == 0 {
        return Err(
            "duration rounds to zero frames; increase duration to at least half a sample".into(),
        );
    }
    let params = request.patch.to_params()?;
    let mut voice = Voice::new(f64::from(request.sample_rate), request.seed)?;
    voice.set_params(params)?;
    voice.note_on(request.frequency, request.velocity)?;
    let output = File::create(output_path)
        .map_err(|error| format!("cannot create WAV {}: {error}", output_path.display()))?;
    let mut buffered = BufWriter::new(output);
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: request.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut wav = hound::WavWriter::new(&mut buffered, spec)?;
    let mut peak = 0.0_f64;
    let mut energy = 0.0_f64;
    for frame_index in 0..frame_count {
        // Note-off occurs before generating the rounded gate sample. A gate
        // equal to duration therefore holds through the final output frame.
        if frame_index == gate_frame {
            voice.note_off();
        }
        for sample in voice.next_frame() {
            if !sample.is_finite() {
                return Err(format!("kernel produced non-finite audio at frame {frame_index}; output WAV is incomplete").into());
            }
            let value = f64::from(sample);
            peak = peak.max(value.abs());
            energy += value * value;
            wav.write_sample(sample)?;
        }
    }
    if gate_frame == frame_count {
        voice.note_off();
    }
    wav.finalize()?;
    buffered.flush()?;
    let metrics = serde_json::json!({
        "frames": frame_count,
        "gate_frame": gate_frame,
        "sample_rate": request.sample_rate,
        "peak": peak,
        "rms": (energy / (frame_count * 2) as f64).sqrt(),
    });
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &metrics)?;
    writeln!(stdout)?;
    Ok(())
}

fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let first = args
        .next()
        .ok_or("usage: plasma-render --describe | plasma-render REQUEST.json OUTPUT.wav")?;
    if first == "--describe" {
        if args.next().is_some() {
            return Err("--describe takes no additional arguments".into());
        }
        return describe();
    }
    if first == "--help" || first == "-h" {
        println!(
            "Usage: plasma-render --describe | plasma-render REQUEST.json OUTPUT.wav\nRenders unclipped float32 stereo WAV; gate and duration round to the nearest frame.\nSample rate: 8000..192000 Hz. Maximum duration: 30 seconds. Maximum request: 65536 bytes.\nRequest velocity: integer 0..127, modulation only.\nRoutes are [AMP ENV, LFO, MOD ENV, Velocity, Key Track], each with {TARGET_COUNT} signed depths in [-1, 1].\nKey Track: MIDI 60 = zero, 60 semitones per unit, clamped to [-1, 1]; depth is normalized target travel."
        );
        return Ok(());
    }
    let output = args
        .next()
        .ok_or("missing OUTPUT.wav; usage: plasma-render REQUEST.json OUTPUT.wav")?;
    if args.next().is_some() {
        return Err("too many arguments; usage: plasma-render REQUEST.json OUTPUT.wav".into());
    }
    render(Path::new(&first), Path::new(&output))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("plasma-render: {error}");
        std::process::exit(1);
    }
}
