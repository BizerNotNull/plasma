use plasma_kernel::{
    LfoWave, OscillatorParams, TARGET_COUNT, Voice, VoiceParams, Waveform, target_range,
};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;

const MAX_REQUEST_BYTES: u64 = 65_536;
const OSC_FIELDS: [&str; 9] = [
    "pitch",
    "fine",
    "phase",
    "phase_random",
    "pulse_width",
    "unison",
    "detune",
    "pan",
    "level",
];
const GLOBAL_FIELDS: [&str; 9] = [
    "volume",
    "attack",
    "decay",
    "sustain",
    "release",
    "lfo_rate",
    "lfo_phase",
    "cutoff",
    "resonance",
];
type Result<T, E = Box<dyn Error>> = std::result::Result<T, E>;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum OscWave {
    Sine,
    Triangle,
    Saw,
    Pulse,
}
impl From<OscWave> for Waveform {
    fn from(value: OscWave) -> Self {
        match value {
            OscWave::Sine => Self::Sine,
            OscWave::Triangle => Self::Triangle,
            OscWave::Saw => Self::Saw,
            OscWave::Pulse => Self::Pulse,
        }
    }
}
impl From<Waveform> for OscWave {
    fn from(value: Waveform) -> Self {
        match value {
            Waveform::Sine => Self::Sine,
            Waveform::Triangle => Self::Triangle,
            Waveform::Saw => Self::Saw,
            Waveform::Pulse => Self::Pulse,
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ModWave {
    Sine,
    Triangle,
    Saw,
    Square,
}
impl From<ModWave> for LfoWave {
    fn from(value: ModWave) -> Self {
        match value {
            ModWave::Sine => Self::Sine,
            ModWave::Triangle => Self::Triangle,
            ModWave::Saw => Self::Saw,
            ModWave::Square => Self::Square,
        }
    }
}
impl From<LfoWave> for ModWave {
    fn from(value: LfoWave) -> Self {
        match value {
            LfoWave::Sine => Self::Sine,
            LfoWave::Triangle => Self::Triangle,
            LfoWave::Saw => Self::Saw,
            LfoWave::Square => Self::Square,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Patch {
    controls: Vec<f64>,
    waveforms: [OscWave; 3],
    lfo_wave: ModWave,
    lfo_retrigger: bool,
    routes: [Vec<f64>; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    patch: Patch,
    sample_rate: u32,
    frequency: f64,
    duration: f64,
    gate: f64,
    seed: u64,
}

#[derive(Serialize)]
struct Control {
    name: String,
    min: f64,
    max: f64,
    log: bool,
}

fn control_name(index: usize) -> String {
    if index < 27 {
        format!("osc{}_{}", index / 9 + 1, OSC_FIELDS[index % 9])
    } else {
        GLOBAL_FIELDS[index - 27].to_owned()
    }
}

impl Patch {
    fn from_params(params: &VoiceParams) -> Self {
        let mut controls = Vec::with_capacity(TARGET_COUNT);
        for osc in &params.oscillators {
            controls.extend_from_slice(&[
                osc.pitch,
                osc.fine,
                osc.phase,
                osc.phase_random,
                osc.pulse_width,
                f64::from(osc.unison),
                osc.detune,
                osc.pan,
                osc.level,
            ]);
        }
        controls.push(f64::from(params.volume));
        controls.extend(params.globals.iter().map(|&value| f64::from(value)));
        Self {
            controls,
            waveforms: params.oscillators.map(|osc| osc.waveform.into()),
            lfo_wave: params.lfo_wave.into(),
            lfo_retrigger: params.lfo_retrigger,
            routes: params
                .routes
                .map(|row| row.into_iter().map(f64::from).collect()),
        }
    }

    fn to_params(&self) -> Result<VoiceParams> {
        if self.controls.len() != TARGET_COUNT {
            return Err(
                format!("patch.controls must contain exactly {TARGET_COUNT} numbers").into(),
            );
        }
        for (index, &value) in self.controls.iter().enumerate() {
            let (min, max, _) = target_range(index)?;
            // Use the same shortest decimal bounds emitted by --describe, rather
            // than widening f32 binary error into the native f64 oscillator API.
            let lower: f64 = min.to_string().parse()?;
            let upper: f64 = max.to_string().parse()?;
            if !value.is_finite() || !(lower..=upper).contains(&value) {
                return Err(format!(
                    "patch.controls[{index}] ({}) must be finite in [{min}, {max}], got {value}",
                    control_name(index),
                )
                .into());
            }
            if index < 27 && index % 9 == 5 && value.fract() != 0.0 {
                return Err(format!(
                    "patch.controls[{index}] ({}) must be an integer",
                    control_name(index)
                )
                .into());
            }
        }
        let mut params = VoiceParams::default();
        for (index, osc) in params.oscillators.iter_mut().enumerate() {
            let c = &self.controls[index * 9..index * 9 + 9];
            *osc = OscillatorParams {
                waveform: self.waveforms[index].into(),
                pitch: c[0],
                fine: c[1],
                phase: c[2],
                phase_random: c[3],
                pulse_width: c[4],
                unison: c[5] as u8,
                detune: c[6],
                pan: c[7],
                level: c[8],
            };
        }
        params.volume = self.controls[27] as f32;
        for (index, value) in params.globals.iter_mut().enumerate() {
            *value = self.controls[28 + index] as f32;
        }
        params.lfo_wave = self.lfo_wave.into();
        params.lfo_retrigger = self.lfo_retrigger;
        for (source, row) in self.routes.iter().enumerate() {
            if row.len() != TARGET_COUNT {
                return Err(format!(
                    "patch.routes[{source}] must contain exactly {TARGET_COUNT} depths"
                )
                .into());
            }
            for (target, &depth) in row.iter().enumerate() {
                if !depth.is_finite() || !(-1.0..=1.0).contains(&depth) {
                    return Err(format!(
                        "patch.routes[{source}][{target}] must be finite in [-1, 1], got {depth}"
                    )
                    .into());
                }
                params.routes[source][target] = depth as f32;
            }
        }
        params.validate()?;
        Ok(params)
    }
}

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
    voice.note_on(request.frequency)?;
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
            "Usage: plasma-render --describe | plasma-render REQUEST.json OUTPUT.wav\nRenders unclipped float32 stereo WAV; gate and duration round to the nearest frame.\nSample rate: 8000..192000 Hz. Maximum duration: 30 seconds. Maximum request: 65536 bytes.\nRoutes are [ENV, LFO], each with 36 signed depths in [-1, 1]."
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
