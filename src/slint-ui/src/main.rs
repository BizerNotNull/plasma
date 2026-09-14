use std::{rc::Rc, time::Duration};

use plasma_api::{FilterMode, LfoWave, Synth, TARGET_COUNT};
use slint::{ComponentHandle, Model, VecModel};

slint::include_modules!();

mod audio;
mod controls;
mod error;
mod performance;

use audio::DeviceEvent;
use controls::{edit_parameter, oscillator_state, release_audition, show_result};
use error::{Error, log_error};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let performance = Rc::new(performance::Performance::new()?);
    let synth = Synth::new();
    performance.mark("synth_ready_ms");
    let audio_start = if performance.no_audio {
        Err("Disabled for performance isolation".into())
    } else {
        audio::AudioWorker::start(synth.clone())
    };
    let window = MainWindow::new()?;
    performance.mark("window_created_ms");
    let initial = (0..3)
        .map(|index| synth.params(index).map(oscillator_state))
        .collect::<Result<Vec<_>, _>>()?;
    let oscillators = Rc::new(VecModel::from(initial));
    window.set_oscillators(oscillators.clone().into());
    let held_notes = Rc::new(VecModel::from(vec![false; 13]));
    window.set_held_notes(held_notes.clone().into());
    // The API contract initializes master gain to 0.25; establish it explicitly
    // so the control and engine cannot start with different values.
    synth.set_volume(0.25)?;
    window.set_volume(25.0);
    let params = synth.voice_params()?;
    let globals = Rc::new(VecModel::from(params.globals.to_vec()));
    let env_depths = Rc::new(VecModel::from(params.routes[0].to_vec()));
    let lfo_depths = Rc::new(VecModel::from(params.routes[1].to_vec()));
    let mod_env_depths = Rc::new(VecModel::from(params.routes[2].to_vec()));
    let velocity_depths = Rc::new(VecModel::from(params.routes[3].to_vec()));
    let key_track_depths = Rc::new(VecModel::from(params.routes[4].to_vec()));
    let effective = Rc::new(VecModel::from(params.normalized().to_vec()));
    window.set_globals(globals.clone().into());
    window.set_env_depths(env_depths.clone().into());
    window.set_lfo_depths(lfo_depths.clone().into());
    window.set_mod_env_depths(mod_env_depths.clone().into());
    window.set_velocity_depths(velocity_depths.clone().into());
    window.set_key_track_depths(key_track_depths.clone().into());
    window.set_effective_values(effective.clone().into());
    window.set_lfo_wave(0);
    window.set_lfo_retrigger(params.lfo_retrigger);
    window.set_filter_mode(0);
    window.on_global_edited({
        let weak = window.as_weak();
        let synth = synth.clone();
        move |index, value| {
            let Some(window) = weak.upgrade() else { return };
            let result = usize::try_from(index)
                .map_err(|_| Error::from("Invalid global index"))
                .and_then(|index| synth.set_global(index, value).map_err(Error::from));
            show_result(&window, result);
            if let Ok(params) = synth.voice_params() {
                for (i, value) in params.globals.into_iter().enumerate() {
                    if globals.row_data(i) != Some(value) {
                        globals.set_row_data(i, value);
                    }
                }
            }
        }
    });
    window.global::<Modulation>().on_routed({
        let weak = window.as_weak();
        let synth = synth.clone();
        move |target, source, depth| {
            let Some(window) = weak.upgrade() else { return };
            let result = match (usize::try_from(target), usize::try_from(source)) {
                (Ok(target), Ok(source)) => {
                    synth.set_route(target, source, depth).map_err(Error::from)
                }
                _ => Err("Invalid modulation source or target".into()),
            };
            show_result(&window, result);
            if let Ok(params) = synth.voice_params() {
                for i in 0..TARGET_COUNT {
                    if env_depths.row_data(i) != Some(params.routes[0][i]) {
                        env_depths.set_row_data(i, params.routes[0][i]);
                    }
                    if lfo_depths.row_data(i) != Some(params.routes[1][i]) {
                        lfo_depths.set_row_data(i, params.routes[1][i]);
                    }
                    if mod_env_depths.row_data(i) != Some(params.routes[2][i]) {
                        mod_env_depths.set_row_data(i, params.routes[2][i]);
                    }
                    if velocity_depths.row_data(i) != Some(params.routes[3][i]) {
                        velocity_depths.set_row_data(i, params.routes[3][i]);
                    }
                    if key_track_depths.row_data(i) != Some(params.routes[4][i]) {
                        key_track_depths.set_row_data(i, params.routes[4][i]);
                    }
                }
            }
        }
    });
    window.on_lfo_wave_edited({
        let weak = window.as_weak();
        let synth = synth.clone();
        move |wave| {
            let Some(window) = weak.upgrade() else { return };
            let result = match wave {
                0 => Ok(LfoWave::Sine),
                1 => Ok(LfoWave::Triangle),
                2 => Ok(LfoWave::Saw),
                3 => Ok(LfoWave::Square),
                _ => Err(Error::from("Unknown LFO waveform")),
            }
            .and_then(|wave| synth.set_lfo_wave(wave).map_err(Error::from));
            if show_result(&window, result) {
                window.set_lfo_wave(wave);
            }
        }
    });
    window.on_lfo_retrigger_edited({
        let weak = window.as_weak();
        let synth = synth.clone();
        move |retrigger| {
            let Some(window) = weak.upgrade() else { return };
            if show_result(&window, synth.set_lfo_retrigger(retrigger)) {
                window.set_lfo_retrigger(retrigger);
            }
        }
    });
    window.on_filter_mode_edited({
        let weak = window.as_weak();
        let synth = synth.clone();
        move |mode| {
            let Some(window) = weak.upgrade() else { return };
            let result = match mode {
                0 => Ok(FilterMode::Lowpass),
                1 => Ok(FilterMode::Bandpass),
                2 => Ok(FilterMode::Highpass),
                _ => Err(Error::from("Unknown filter mode")),
            }
            .and_then(|mode| synth.set_filter_mode(mode).map_err(Error::from));
            if show_result(&window, result) {
                window.set_filter_mode(mode);
            }
        }
    });
    let modulation_monitor = slint::Timer::default();
    modulation_monitor.start(slint::TimerMode::Repeated, Duration::from_millis(33), {
        let weak = window.as_weak();
        let synth = synth.clone();
        let mut previous = synth.telemetry();
        window.set_velocity_value(previous.velocity);
        window.set_key_track_value(previous.key_track);
        move || {
            let Some(window) = weak.upgrade() else { return };
            let telemetry = synth.telemetry();
            if telemetry.env != previous.env {
                window.set_env_value(telemetry.env);
            }
            if telemetry.lfo != previous.lfo {
                window.set_lfo_value(telemetry.lfo);
            }
            if telemetry.mod_env != previous.mod_env {
                window.set_mod_env_value(telemetry.mod_env);
            }
            if telemetry.velocity != previous.velocity {
                window.set_velocity_value(telemetry.velocity);
            }
            if telemetry.key_track != previous.key_track {
                window.set_key_track_value(telemetry.key_track);
            }
            for (i, value) in telemetry.effective.into_iter().enumerate() {
                if value != previous.effective[i] {
                    effective.set_row_data(i, value);
                }
            }
            previous = telemetry;
        }
    });

    performance.mark("controls_ready_ms");
    let audio = match audio_start {
        Ok(output) => Some(Rc::new(output)),
        Err(error) => {
            window.set_audio_status(
                format!("Audio unavailable: {error}. Connect or enable an output device, then restart PLASMA. You can still edit the instrument.").into(),
            );
            None
        }
    };
    performance.mark("audio_dispatched_ms");

    window.on_parameter_edited({
        let weak = window.as_weak();
        let synth = synth.clone();
        move |index, field, value| {
            let Some(window) = weak.upgrade() else { return };
            let Ok(index) = usize::try_from(index) else {
                show_result(&window, Err(Error::from("Invalid oscillator index")));
                return;
            };
            show_result(&window, edit_parameter(&synth, index, field, value));
            // Read back accepted values, including after a rejected edit.
            match synth.params(index) {
                Ok(params) => oscillators.set_row_data(index, oscillator_state(params)),
                Err(error) => {
                    window.set_control_error(error.to_string().into());
                }
            }
        }
    });
    window.on_volume_edited({
        let weak = window.as_weak();
        let synth = synth.clone();
        move |value| {
            if let Some(window) = weak.upgrade() {
                if show_result(&window, synth.set_volume(value / 100.0)) {
                    window.set_volume(value);
                }
            }
        }
    });
    window.on_audition({
        let weak = window.as_weak();
        let synth = synth.clone();
        let held_notes = held_notes.clone();
        move |note| {
            let Some(window) = weak.upgrade() else { return };
            if !window.get_audio_ready() {
                show_result(&window, Err(Error::from("Audio output is unavailable")));
                return;
            }
            if !(60..=72).contains(&note) {
                show_result(&window, Err(Error::from("Audition note must be in C4–C5")));
                return;
            }
            let index = (note - 60) as usize;
            let held = held_notes.row_data(index).unwrap_or(false);
            let result = if held {
                synth.note_off(note as u8)
            } else {
                synth.note_on(note as u8, window.get_velocity().clamp(1, 127) as u8)
            };
            match result {
                Ok(()) => {
                    held_notes.set_row_data(index, !held);
                    show_result(&window, Ok::<(), Error>(()));
                }
                Err(error) => {
                    // A rejected event may panic-release the engine; reconcile all keys.
                    let result = match release_audition(&synth, &held_notes) {
                        Ok(()) => Err(Error::from(error)),
                        Err(release_error) => Err(format!(
                            "{error}; could not release all notes: {release_error}"
                        )
                        .into()),
                    };
                    show_result(&window, result);
                }
            }
        }
    });
    window.on_stop({
        let weak = window.as_weak();
        let synth = synth.clone();
        let held_notes = held_notes.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                show_result(&window, release_audition(&synth, &held_notes));
            }
        }
    });

    let audio_monitor = slint::Timer::default();
    if let Some(output) = audio.as_ref() {
        let output = Rc::clone(output);
        let weak = window.as_weak();
        let synth = synth.clone();
        let performance = performance.clone();
        let held_notes = held_notes.clone();
        audio_monitor.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
            let Some(window) = weak.upgrade() else { return };
            while let Ok(event) = output.events.try_recv() {
                match event {
                    DeviceEvent::Ready(description) => {
                        performance.mark("audio_ready_ms");
                        window.set_audio_status(description.into());
                        window.set_audio_ready(true);
                    }
                    DeviceEvent::Failed(error) => {
                        performance.mark("audio_failed_ms");
                        window.set_audio_ready(false);
                        window.set_audio_status(format!("Audio unavailable: {error}. Check the output device and restart PLASMA.").into());
                        show_result(&window, release_audition(&synth, &held_notes));
                    }
                }
            }
        });
    }

    performance.attach(&window)?;
    performance.mark("event_loop_enter_ms");
    let result = window.run();
    audio_monitor.stop();
    drop(audio_monitor);
    if let Err(error) = release_audition(&synth, &held_notes) {
        log_error(format!(
            "could not stop the instrument during shutdown: {error}"
        ));
    }
    // Keep the real device stream alive throughout the event loop, then stop it.
    drop(audio);
    result?;
    performance.report()?;
    Ok(())
}
