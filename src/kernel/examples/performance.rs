//! Reproducible callback-cost benchmark (release mode; no audio device required).
//! cargo run --release --manifest-path src/kernel/Cargo.toml --example performance
//! Optional: --iterations 512 --batches 3 --warmup 128 --scenario voice_dense
//! Each batch starts from the same seed/state, then warms up before sampling.
//! One sample is one render callback, including the named control operation.
//! Buffers, parameter variants and timings are allocated before measurement;
//! output validation, checksums, sorting and CSV output happen outside timing.
//! Budget overruns are simulated callback deadlines, not observed device xruns.
use plasma_kernel::{OscillatorBank, SOURCE_COUNT, TARGET_COUNT, Voice, VoiceParams, Waveform};
use std::{error::Error, hint::black_box, time::Instant};

const SEED: u64 = 0x706c_6173_6d61_0042;

type Result<T, E = Box<dyn Error>> = std::result::Result<T, E>;

#[derive(Clone, Copy)]
enum Operation {
    Render,
    Retrigger,
    RepeatParams,
    ChangeParams,
}

#[derive(Clone, Copy)]
enum Patch {
    Default,
    Stack(Waveform),
    Sparse,
    Dense,
}

#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    bank: bool,
    idle: bool,
    patch: Patch,
    operation: Operation,
    rate: u32,
    frames: usize,
}

struct Options {
    iterations: usize,
    batches: usize,
    warmup: usize,
    filter: String,
}

impl Options {
    fn parse() -> Result<Self> {
        let mut options = Self {
            iterations: 512,
            batches: 3,
            warmup: 128,
            filter: String::new(),
        };
        let mut args = std::env::args().skip(1);
        while let Some(key) = args.next() {
            let value = args.next().ok_or("each option requires a value")?;
            match key.as_str() {
                "--iterations" => options.iterations = value.parse()?,
                "--batches" => options.batches = value.parse()?,
                "--warmup" => options.warmup = value.parse()?,
                "--scenario" => options.filter = value,
                _ => return Err(format!("unknown option: {key}").into()),
            }
        }
        if options.iterations == 0 || options.batches == 0 || options.warmup == 0 {
            return Err("iterations, batches and warmup must be positive".into());
        }
        Ok(options)
    }
}

fn parameters(patch: Patch) -> [VoiceParams; 2] {
    let mut params = VoiceParams::default();
    if !matches!(patch, Patch::Default) {
        let waveform = match patch {
            Patch::Stack(waveform) => waveform,
            _ => Waveform::Saw,
        };
        for (i, oscillator) in params.oscillators.iter_mut().enumerate() {
            oscillator.waveform = waveform;
            oscillator.unison = 4;
            oscillator.detune = 12.0;
            oscillator.phase_random = 0.75;
            oscillator.pan = (i as f64 - 1.0) * 0.3;
            oscillator.level = 0.25;
        }
    }
    if matches!(patch, Patch::Sparse | Patch::Dense) {
        params.globals[6] = 3200.0;
        params.globals[4] = 3.0;
        params.routes[0][34] = 0.15;
        params.routes[1][7] = 0.2;
        params.routes[1][18] = 0.04;
    }
    if matches!(patch, Patch::Dense) {
        for source in 0..SOURCE_COUNT {
            for target in 0..TARGET_COUNT {
                // All sources route to every destination. Small signed depths
                // keep the patch audible while exercising clamp/round/log paths.
                params.routes[source][target] = if (source + target) % 2 == 0 {
                    0.035
                } else {
                    -0.025
                };
            }
        }
    }
    let mut changed = params;
    changed.oscillators[0].fine = 17.0;
    changed.oscillators[1].pan = 0.35;
    changed.globals[6] = 1400.0;
    [params, changed]
}

enum Renderer {
    Bank(OscillatorBank),
    Voice(Voice),
}

impl Renderer {
    fn new(scenario: Scenario, params: VoiceParams) -> Result<Self> {
        if scenario.bank {
            let mut bank = OscillatorBank::new(f64::from(scenario.rate), SEED)?;
            for (i, oscillator) in params.oscillators.into_iter().enumerate() {
                bank.set_params(i, oscillator)?;
            }
            if !scenario.idle {
                bank.note_on(220.0)?;
            }
            Ok(Self::Bank(bank))
        } else {
            let mut voice = Voice::new(f64::from(scenario.rate), SEED)?;
            voice.set_params(params)?;
            if !scenario.idle {
                voice.note_on(220.0, 127)?;
            }
            Ok(Self::Voice(voice))
        }
    }

    fn callback(
        &mut self,
        scenario: Scenario,
        params: &[VoiceParams; 2],
        index: usize,
        output: &mut [[f32; 2]],
    ) -> Result<()> {
        let variant = if matches!(scenario.operation, Operation::ChangeParams) {
            index % 2
        } else {
            0
        };
        match self {
            Self::Bank(bank) => {
                match scenario.operation {
                    Operation::Render => {}
                    Operation::Retrigger => {
                        bank.note_on(if index % 2 == 0 { 220.0 } else { 329.6275569 })?
                    }
                    Operation::RepeatParams | Operation::ChangeParams => {
                        for (i, oscillator) in params[variant].oscillators.iter().enumerate() {
                            bank.set_params(i, black_box(*oscillator))?;
                        }
                    }
                }
                bank.render(black_box(output));
            }
            Self::Voice(voice) => {
                match scenario.operation {
                    Operation::Render => {}
                    Operation::Retrigger => {
                        voice.note_off();
                        voice.note_on(if index % 2 == 0 { 220.0 } else { 329.6275569 }, 127)?;
                    }
                    Operation::RepeatParams | Operation::ChangeParams => {
                        voice.set_params(black_box(params[variant]))?;
                    }
                }
                voice.render(black_box(output));
            }
        }
        Ok(())
    }
}

fn quantile(sorted: &[f64], percent: usize) -> f64 {
    sorted[(sorted.len() * percent).div_ceil(100) - 1]
}

fn measure(scenario: Scenario, options: &Options, batch: usize) -> Result<()> {
    let params = parameters(scenario.patch);
    let mut renderer = Renderer::new(scenario, params[0])?;
    let mut output = vec![[0.0_f32; 2]; scenario.frames];
    let mut timings = vec![0.0_f64; options.iterations];
    for index in 0..options.warmup {
        renderer.callback(scenario, &params, index, &mut output)?;
        black_box(&output);
    }
    let mut energy = 0.0_f64;
    let mut checksum = 0.0_f64;
    let budget_us = scenario.frames as f64 / f64::from(scenario.rate) * 1e6;
    let mut overruns = 0;
    for (index, timing) in timings.iter_mut().enumerate() {
        let start = Instant::now();
        let result = renderer.callback(scenario, &params, options.warmup + index, &mut output);
        black_box(&output);
        *timing = start.elapsed().as_secs_f64() * 1e6;
        result?;
        overruns += usize::from(*timing > budget_us);
        // Validate the actual rendered signal, outside the measured callback.
        for frame in &output {
            for sample in frame {
                if !sample.is_finite() {
                    return Err(format!("{} rendered non-finite audio", scenario.name).into());
                }
                energy += f64::from(*sample) * f64::from(*sample);
                checksum += f64::from(*sample);
            }
        }
    }
    if (scenario.idle && energy != 0.0) || (!scenario.idle && energy == 0.0) {
        return Err(format!("{} rendered unexpected silence/non-silence", scenario.name).into());
    }
    let mean = timings.iter().sum::<f64>() / timings.len() as f64;
    timings.sort_unstable_by(f64::total_cmp);
    let p50 = quantile(&timings, 50);
    let p95 = quantile(&timings, 95);
    let p99 = quantile(&timings, 99);
    let max = timings[timings.len() - 1];
    println!(
        "{},{batch},{},{},{},{},{p50:.3},{p95:.3},{p99:.3},{max:.3},{mean:.3},{budget_us:.3},{:.4},{overruns},{energy:.9},{checksum:.9}",
        scenario.name,
        scenario.rate,
        scenario.frames,
        options.iterations,
        options.warmup,
        p99 / budget_us * 100.0,
    );
    Ok(())
}

fn main() -> Result<()> {
    let options = Options::parse()?;
    let base = Scenario {
        name: "voice_default",
        bank: false,
        idle: false,
        patch: Patch::Default,
        operation: Operation::Render,
        rate: 48_000,
        frames: 256,
    };
    let mut scenarios = vec![
        Scenario {
            name: "voice_idle",
            idle: true,
            ..base
        },
        base,
        Scenario {
            name: "bank_idle",
            bank: true,
            idle: true,
            ..base
        },
        Scenario {
            name: "bank_default",
            bank: true,
            ..base
        },
    ];
    for (voice_name, bank_name, waveform) in [
        ("voice_3x4_sine", "bank_3x4_sine", Waveform::Sine),
        (
            "voice_3x4_triangle",
            "bank_3x4_triangle",
            Waveform::Triangle,
        ),
        ("voice_3x4_saw", "bank_3x4_saw", Waveform::Saw),
        ("voice_3x4_pulse", "bank_3x4_pulse", Waveform::Pulse),
    ] {
        scenarios.push(Scenario {
            name: voice_name,
            patch: Patch::Stack(waveform),
            ..base
        });
        scenarios.push(Scenario {
            name: bank_name,
            bank: true,
            patch: Patch::Stack(waveform),
            ..base
        });
    }
    for (name, patch, operation, bank) in [
        ("voice_sparse", Patch::Sparse, Operation::Render, false),
        ("voice_dense", Patch::Dense, Operation::Render, false),
        (
            "voice_retrigger",
            Patch::Stack(Waveform::Saw),
            Operation::Retrigger,
            false,
        ),
        (
            "voice_repeat_params",
            Patch::Stack(Waveform::Saw),
            Operation::RepeatParams,
            false,
        ),
        (
            "voice_change_params",
            Patch::Stack(Waveform::Saw),
            Operation::ChangeParams,
            false,
        ),
        (
            "bank_repeat_params",
            Patch::Stack(Waveform::Saw),
            Operation::RepeatParams,
            true,
        ),
        (
            "bank_change_params",
            Patch::Stack(Waveform::Saw),
            Operation::ChangeParams,
            true,
        ),
    ] {
        scenarios.push(Scenario {
            name,
            patch,
            operation,
            bank,
            ..base
        });
    }
    // Representative rate/buffer corners rather than a Cartesian product.
    for (rate, frames) in [(44_100, 256), (48_000, 64), (96_000, 1024)] {
        scenarios.push(Scenario {
            rate,
            frames,
            ..base
        });
    }
    for (rate, frames) in [(44_100, 1024), (96_000, 64)] {
        scenarios.push(Scenario {
            name: "voice_dense",
            patch: Patch::Dense,
            rate,
            frames,
            ..base
        });
    }
    let selected: Vec<_> = scenarios
        .into_iter()
        .filter(|scenario| scenario.name.contains(&options.filter))
        .collect();
    if selected.is_empty() {
        return Err(format!("no scenario matches {:?}", options.filter).into());
    }
    println!(
        "scenario,batch,sample_rate,frames,iterations,warmup,p50_us,p95_us,p99_us,max_us,mean_us,budget_us,p99_budget_percent,overruns,energy,checksum"
    );
    for batch in 1..=options.batches {
        for scenario in &selected {
            measure(*scenario, &options, batch)?;
        }
    }
    Ok(())
}
