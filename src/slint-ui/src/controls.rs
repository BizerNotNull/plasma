use crate::error::Error;
use crate::{MainWindow, OscillatorState};
use plasma_api::{OscillatorParams, Synth, Waveform};
use slint::{Model, VecModel};

pub fn oscillator_state(params: OscillatorParams, sync: bool, fm: f32, ring: f32) -> OscillatorState {
    OscillatorState {
        waveform: match params.waveform {
            Waveform::Sine => 0,
            Waveform::Triangle => 1,
            Waveform::Saw => 2,
            Waveform::Pulse => 3,
        },
        pitch: params.pitch as f32,
        fine: params.fine as f32,
        phase: (params.phase * 360.0) as f32,
        phase_random: (params.phase_random * 100.0) as f32,
        pulse_width: (params.pulse_width * 100.0) as f32,
        unison: f32::from(params.unison),
        detune: params.detune as f32,
        pan: (params.pan * 100.0) as f32,
        level: (params.level * 100.0) as f32,
        sync,
        fm: fm * 100.0,
        ring: ring * 100.0,
    }
}

pub fn edit_parameter(synth: &Synth, index: usize, field: i32, value: f32) -> Result<(), Error> {
    if !value.is_finite() {
        return Err("Parameter value must be finite".into());
    }
    if field == 10 {
        return Ok(synth.set_sync(index, value != 0.0)?);
    }
    if field == 11 {
        return Ok(synth.set_fm(index, value / 100.0)?);
    }
    if field == 12 {
        return Ok(synth.set_ring(index, value / 100.0)?);
    }
    let mut params = synth.params(index)?;
    let value = f64::from(value);
    match field {
        0 => {
            params.waveform = match value {
                0.0 => Waveform::Sine,
                1.0 => Waveform::Triangle,
                2.0 => Waveform::Saw,
                3.0 => Waveform::Pulse,
                _ => return Err("Unknown waveform".into()),
            };
        }
        1 => params.pitch = value.round(),
        2 => params.fine = value,
        3 => params.phase = value / 360.0,
        4 => params.phase_random = value / 100.0,
        5 => params.pulse_width = value / 100.0,
        6 => {
            if !(1.0..=4.0).contains(&value) {
                return Err("Unison must be between one and four voices".into());
            }
            params.unison = value.round() as u8;
        }
        7 => params.detune = value,
        8 => params.pan = value / 100.0,
        9 => params.level = value / 100.0,
        _ => return Err("Unknown oscillator parameter".into()),
    }
    Ok(synth.set_params(index, params)?)
}

pub fn show_result(window: &MainWindow, result: Result<(), impl std::fmt::Display>) -> bool {
    match result {
        Ok(()) => {
            window.set_control_error("".into());
            true
        }
        Err(error) => {
            window.set_control_error(error.to_string().into());
            false
        }
    }
}

pub fn release_audition(synth: &Synth, held_notes: &VecModel<bool>) -> Result<(), Error> {
    for index in 0..held_notes.row_count() {
        held_notes.set_row_data(index, false);
    }
    Ok(synth.all_notes_off()?)
}
