//! Offline by default. Run with --audio to also open the default output device.
//! CSV quantiles are nearest-rank quantiles of per-operation batch averages,
//! except creation and audio rows, which time individual operations.
use plasma_api::{AudioOutput, LfoWave, OscillatorParams, Synth, TARGET_COUNT};
use std::hint::black_box;
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

const SAMPLES: usize = 1_000;
const BATCH: usize = 32;
const WARMUP: usize = 1_024;
const AUDIO_REPEATS: usize = 8;
const SEED: u64 = 0x706c_6173_6d61_2026;

#[derive(Clone, Copy)]
struct Input {
    oscillator: OscillatorParams,
    unit: f32,
    note: u8,
    target: usize,
    source: usize,
}

fn inputs() -> [Input; 256] {
    let mut seed = SEED;
    std::array::from_fn(|i| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let unit = (seed >> 40) as f32 / 16_777_216.0;
        Input {
            oscillator: OscillatorParams {
                pitch: f64::from(unit) * 24.0 - 12.0,
                fine: f64::from(unit) * 100.0 - 50.0,
                ..OscillatorParams::default()
            },
            unit,
            note: 36 + (seed % 60) as u8,
            target: i % TARGET_COUNT,
            source: (i / TARGET_COUNT) % 2,
        }
    })
}

fn report(name: &str, samples: &mut [f64], errors: usize) {
    samples.sort_unstable_by(f64::total_cmp);
    if samples.is_empty() {
        println!("{name},0,,,,,{errors}");
        return;
    }
    let percentile = |p: usize| samples[(samples.len() * p).div_ceil(100) - 1];
    println!(
        "{name},{},{:.6},{:.6},{:.6},{:.6},{errors}",
        samples.len(),
        percentile(50),
        percentile(95),
        percentile(99),
        samples[samples.len() - 1],
    );
}

fn measure(
    name: &str,
    batch: usize,
    mut operation: impl FnMut(usize) -> Result<(), String>,
) -> Result<(), String> {
    for i in 0..WARMUP {
        operation(i)?;
    }
    let mut samples = vec![0.0; SAMPLES];
    for (sample, elapsed) in samples.iter_mut().enumerate() {
        let start = Instant::now();
        for offset in 0..batch {
            operation(black_box(sample * batch + offset))?;
        }
        *elapsed = start.elapsed().as_secs_f64() * 1_000_000.0 / batch as f64;
    }
    report(name, &mut samples, 0);
    Ok(())
}

// The background worker runs throughout both warmup and sampling; thread setup,
// synchronization, sample storage and reporting are outside measured intervals.
fn concurrent(synth: &Synth, data: &[Input; 256], writer: bool) -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(Barrier::new(2));
    let worker_synth = synth.clone();
    let worker_stop = stop.clone();
    let worker_ready = ready.clone();
    let worker_data = *data;
    let worker = thread::spawn(move || -> Result<(), String> {
        let mut index = 0;
        worker_ready.wait();
        while !worker_stop.load(Ordering::Relaxed) {
            if writer {
                worker_synth.set_volume(worker_data[index & 255].unit)?;
            } else {
                black_box(worker_synth.telemetry());
            }
            index = index.wrapping_add(1);
        }
        Ok(())
    });
    ready.wait();
    let result = if writer {
        measure("telemetry_with_control_writer", BATCH, |_| {
            black_box(synth.telemetry());
            Ok(())
        })
    } else {
        measure("set_volume_with_telemetry_reader", BATCH, |i| {
            synth.set_volume(data[i & 255].unit)
        })
    };
    stop.store(true, Ordering::Relaxed);
    worker
        .join()
        .map_err(|_| "Concurrent worker panicked".to_owned())??;
    result
}

fn offline() -> Result<(), String> {
    let data = inputs();
    measure("timer_loop_floor", BATCH, |i| {
        black_box(i);
        Ok(())
    })?;
    // Allocation and deallocation are deliberately included in this row.
    measure("synth_create_drop_allocating", 1, |_| {
        drop(black_box(Synth::new()));
        Ok(())
    })?;
    let synth = Synth::new();
    let oscillator_count = synth.voice_params()?.oscillators.len();
    measure("voice_params_read", BATCH, |_| {
        black_box(synth.voice_params()?);
        Ok(())
    })?;
    measure("oscillator_params_read", BATCH, |i| {
        black_box(synth.params(i % oscillator_count)?);
        Ok(())
    })?;
    measure("oscillator_params_write", BATCH, |i| {
        synth.set_params(i % oscillator_count, data[i & 255].oscillator)
    })?;
    measure("set_volume", BATCH, |i| {
        synth.set_volume(data[i & 255].unit)
    })?;
    measure("set_global_sustain", BATCH, |i| {
        synth.set_global(2, data[i & 255].unit)
    })?;
    measure("set_lfo_wave", BATCH, |i| {
        synth.set_lfo_wave(match i % 4 {
            0 => LfoWave::Sine,
            1 => LfoWave::Triangle,
            2 => LfoWave::Saw,
            _ => LfoWave::Square,
        })
    })?;
    measure("set_lfo_retrigger", BATCH, |i| {
        synth.set_lfo_retrigger(i % 2 == 0)
    })?;
    measure("set_route", BATCH, |i| {
        let input = data[i & 255];
        synth.set_route(input.target, input.source, input.unit * 2.0 - 1.0)
    })?;
    measure("note_on", BATCH, |i| synth.note_on(data[i & 255].note))?;
    measure("note_off", BATCH, |_| synth.note_off())?;
    measure("note_on_off_pair", BATCH, |i| {
        synth.note_on(data[i & 255].note)?;
        synth.note_off()
    })?;
    measure("telemetry", BATCH, |_| {
        black_box(synth.telemetry());
        Ok(())
    })?;
    concurrent(&synth, &data, true)?;
    concurrent(&synth, &data, false)?;
    Ok(())
}

fn audio_attempt(name: &str, count: usize) -> usize {
    let mut starts = Vec::with_capacity(count);
    let mut drops = Vec::with_capacity(count);
    let mut failed_starts = Vec::with_capacity(count);
    let mut errors = 0;
    for iteration in 0..count {
        // Always fresh, silent controls: offline note tests never reach a device.
        let synth = Synth::new();
        let start = Instant::now();
        let output = AudioOutput::start(synth);
        let start_us = start.elapsed().as_secs_f64() * 1_000_000.0;
        match output {
            Ok(output) => {
                // Observe asynchronous backend errors, not just successful play().
                // This dwell and all diagnostic formatting are excluded from timing.
                thread::sleep(Duration::from_millis(120));
                let error = output.error();
                eprintln!("{name} attempt {}: {}", iteration + 1, output.description());
                let drop_start = Instant::now();
                drop(output);
                let drop_us = drop_start.elapsed().as_secs_f64() * 1_000_000.0;
                if let Some(error) = error {
                    errors += 1;
                    failed_starts.push(start_us);
                    eprintln!("{name} attempt {} callback error: {error}", iteration + 1);
                } else {
                    starts.push(start_us);
                    drops.push(drop_us);
                }
            }
            Err(error) => {
                errors += 1;
                failed_starts.push(start_us);
                eprintln!("{name} attempt {} start error: {error}", iteration + 1);
            }
        }
    }
    report(&format!("{name}_start_allocating"), &mut starts, errors);
    report(&format!("{name}_drop_deallocating"), &mut drops, errors);
    report(
        &format!("{name}_failed_start_attempt"),
        &mut failed_starts,
        errors,
    );
    errors
}

fn main() -> Result<(), String> {
    let mut audio = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--audio" => audio = true,
            "--help" | "-h" => {
                eprintln!(
                    "Usage: cargo run --release --manifest-path src/api/Cargo.toml --example performance -- [--audio]"
                );
                eprintln!(
                    "Default: offline controls only. --audio: additionally open/close the default output silently (one cold attempt, eight warmed attempts). Any audio failure gives nonzero exit status; success percentiles exclude failed starts and callback errors observed during a 120 ms dwell."
                );
                return Ok(());
            }
            _ => return Err(format!("Unknown argument: {argument}")),
        }
    }
    eprintln!(
        "API performance: seed={SEED:#x}; samples={SAMPLES}; batch={BATCH}; warmup={WARMUP}; nearest-rank quantiles of per-operation batch averages, not individual-call tail latency. iterations counts timed batches; note_on_off_pair counts a pair as one operation. No output/sample allocation inside timing, except explicitly allocating/deallocating creation/audio operations. Fixed input generation precedes timing. API call and loop/Result-check overhead remain included; timer_loop_floor is not subtracted."
    );
    eprintln!(
        "Offline telemetry reads initialized atomics without a DSP publisher; concurrent scenarios measure control writer plus telemetry reader only, not audio callback contention. Audio times include device discovery/configuration, stream construction and play(), not time-to-first-callback or time-to-sound; backend-controlled calls have no hard timeout. First attempt is cold within this process, not necessarily OS/driver cold. No notes are played in audio mode."
    );
    println!("scenario,iterations,p50_us,p95_us,p99_us,max_us,errors");
    offline()?;
    if audio {
        let errors = audio_attempt("audio_cold", 1) + audio_attempt("audio_warm", AUDIO_REPEATS);
        if errors != 0 {
            return Err(format!(
                "{errors} audio attempt(s) failed; see stderr and failed-attempt CSV rows"
            ));
        }
    }
    Ok(())
}
