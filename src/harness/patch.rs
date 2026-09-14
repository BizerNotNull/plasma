use plasma_kernel::{
    GLOBAL_COUNT, LfoWave, OscillatorParams, SOURCE_COUNT, TARGET_COUNT, VoiceParams, Waveform,
    target_range,
};
use serde::{Deserialize, Serialize};
use std::error::Error;

type Result<T, E = Box<dyn Error>> = std::result::Result<T, E>;

pub const OSC_FIELDS: [&str; 9] = [
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

pub const GLOBAL_FIELDS: [&str; GLOBAL_COUNT + 1] = [
    "volume",
    "attack",
    "decay",
    "sustain",
    "release",
    "lfo_rate",
    "lfo_phase",
    "cutoff",
    "resonance",
    "mod_attack",
    "mod_decay",
    "mod_sustain",
    "mod_release",
];

pub const ANALOG_FIELDS: [&str; 13] = [
    "noise",
    "osc1_spread",
    "osc2_spread",
    "osc3_spread",
    "osc2_fm",
    "osc3_fm",
    "osc2_ring",
    "osc3_ring",
    "glide",
    "osc1_fm",
    "osc1_ring",
    "osc2_sync",
    "osc3_sync",
];

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OscWave {
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
pub enum ModWave {
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
pub struct Patch {
    pub controls: Vec<f64>,
    pub waveforms: [OscWave; 3],
    pub lfo_wave: ModWave,
    pub lfo_retrigger: bool,
    pub routes: [Vec<f64>; SOURCE_COUNT],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub patch: Patch,
    pub sample_rate: u32,
    pub frequency: f64,
    pub velocity: u8,
    pub duration: f64,
    pub gate: f64,
    pub seed: u64,
}

#[derive(Serialize)]
pub struct Control {
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub log: bool,
}

pub fn control_name(index: usize) -> String {
    if index < 27 {
        format!("osc{}_{}", index / 9 + 1, OSC_FIELDS[index % 9])
    } else if index < 40 {
        GLOBAL_FIELDS[index - 27].to_owned()
    } else {
        ANALOG_FIELDS[index - 40].to_owned()
    }
}

impl Patch {
    pub fn from_params(params: &VoiceParams) -> Self {
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
        controls.push(f64::from(params.noise));
        controls.extend(params.spread.iter().map(|&value| f64::from(value)));
        controls.push(f64::from(params.fm[1]));
        controls.push(f64::from(params.fm[2]));
        controls.push(f64::from(params.ring[1]));
        controls.push(f64::from(params.ring[2]));
        controls.push(f64::from(params.glide));
        controls.push(f64::from(params.fm[0]));
        controls.push(f64::from(params.ring[0]));
        controls.push(if params.sync[1] { 1.0 } else { 0.0 });
        controls.push(if params.sync[2] { 1.0 } else { 0.0 });
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

    pub fn to_params(&self) -> Result<VoiceParams> {
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
        params.noise = self.controls[40] as f32;
        for (index, value) in params.spread.iter_mut().enumerate() {
            *value = self.controls[41 + index] as f32;
        }
        params.fm[1] = self.controls[44] as f32;
        params.fm[2] = self.controls[45] as f32;
        params.ring[1] = self.controls[46] as f32;
        params.ring[2] = self.controls[47] as f32;
        params.glide = self.controls[48] as f32;
        params.fm[0] = self.controls[49] as f32;
        params.ring[0] = self.controls[50] as f32;
        params.sync[1] = self.controls[51] >= 0.5;
        params.sync[2] = self.controls[52] >= 0.5;
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
