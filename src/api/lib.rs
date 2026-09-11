//! Monophonic synthesis and real system audio. Control writers serialize on a
//! mutex; the audio callback reads a fixed atomic snapshot once per buffer and
//! never locks, waits or allocates. Concurrent updates defer one buffer. Note
//! commands are latest-state controls and can coalesce between audio buffers.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
pub use plasma_kernel::{
    GLOBAL_COUNT, GLOBAL_DEFAULTS, LfoWave, OscillatorParams, TARGET_COUNT, Telemetry, Voice,
    VoiceParams, Waveform, denormalize, normalize, target_range,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Default)]
struct Controls {
    params: VoiceParams,
    note: Option<u8>,
    generation: u64,
}
const WORDS: usize = 115;
struct Published {
    version: AtomicU64,
    words: [AtomicU64; WORDS],
}
impl Default for Published {
    fn default() -> Self {
        let result = Self {
            version: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        };
        result.store(Controls::default());
        result
    }
}
impl Published {
    fn store(&self, c: Controls) {
        let mut words = [0; WORDS];
        let mut n = 0;
        for p in c.params.oscillators {
            for v in [
                p.waveform as u8 as f64,
                p.pitch,
                p.fine,
                p.phase,
                p.phase_random,
                p.pulse_width,
                p.unison as f64,
                p.detune,
                p.pan,
                p.level,
            ] {
                words[n] = v.to_bits();
                n += 1;
            }
        }
        words[n] = (c.params.volume as f64).to_bits();
        n += 1;
        for v in c.params.globals {
            words[n] = (v as f64).to_bits();
            n += 1;
        }
        words[n] = c.params.lfo_wave as u64;
        n += 1;
        words[n] = u64::from(c.params.lfo_retrigger);
        n += 1;
        for v in c.params.routes.into_iter().flatten() {
            words[n] = (v as f64).to_bits();
            n += 1;
        }
        words[n] = c.note.map_or(128, u64::from);
        words[n + 1] = c.generation;
        self.version.fetch_add(1, Ordering::SeqCst);
        for (dst, word) in self.words.iter().zip(words) {
            dst.store(word, Ordering::SeqCst);
        }
        self.version.fetch_add(1, Ordering::SeqCst);
    }
    fn load(&self) -> Option<Controls> {
        let version = self.version.load(Ordering::SeqCst);
        if version & 1 != 0 {
            return None;
        }
        let words: [u64; WORDS] = std::array::from_fn(|i| self.words[i].load(Ordering::SeqCst));
        if self.version.load(Ordering::SeqCst) != version {
            return None;
        }
        let mut c = Controls::default();
        let mut n = 0;
        for p in &mut c.params.oscillators {
            let v: [f64; 10] = std::array::from_fn(|i| f64::from_bits(words[n + i]));
            n += 10;
            *p = OscillatorParams {
                waveform: match v[0] as u8 {
                    1 => Waveform::Triangle,
                    2 => Waveform::Saw,
                    3 => Waveform::Pulse,
                    _ => Waveform::Sine,
                },
                pitch: v[1],
                fine: v[2],
                phase: v[3],
                phase_random: v[4],
                pulse_width: v[5],
                unison: v[6] as u8,
                detune: v[7],
                pan: v[8],
                level: v[9],
            };
        }
        c.params.volume = f64::from_bits(words[n]) as f32;
        n += 1;
        for v in &mut c.params.globals {
            *v = f64::from_bits(words[n]) as f32;
            n += 1;
        }
        c.params.lfo_wave = match words[n] {
            1 => LfoWave::Triangle,
            2 => LfoWave::Saw,
            3 => LfoWave::Square,
            _ => LfoWave::Sine,
        };
        n += 1;
        c.params.lfo_retrigger = words[n] != 0;
        n += 1;
        for v in c.params.routes.iter_mut().flatten() {
            *v = f64::from_bits(words[n]) as f32;
            n += 1;
        }
        c.note = if words[n] < 128 {
            Some(words[n] as u8)
        } else {
            None
        };
        c.generation = words[n + 1];
        Some(c)
    }
}
struct Meters {
    values: [AtomicU32; TARGET_COUNT + 2],
}
impl Default for Meters {
    fn default() -> Self {
        let meters = Self {
            values: std::array::from_fn(|_| AtomicU32::new(0)),
        };
        meters.store(Telemetry::default());
        meters
    }
}
impl Meters {
    fn store(&self, t: Telemetry) {
        self.values[0].store(t.env.to_bits(), Ordering::Relaxed);
        self.values[1].store(t.lfo.to_bits(), Ordering::Relaxed);
        for (dst, v) in self.values[2..].iter().zip(t.effective) {
            dst.store(v.to_bits(), Ordering::Relaxed);
        }
    }
    fn load(&self) -> Telemetry {
        Telemetry {
            env: f32::from_bits(self.values[0].load(Ordering::Relaxed)),
            lfo: f32::from_bits(self.values[1].load(Ordering::Relaxed)),
            effective: std::array::from_fn(|i| {
                f32::from_bits(self.values[i + 2].load(Ordering::Relaxed))
            }),
        }
    }
}
/// Cloneable control handle. Telemetry is real DSP state, sampled per buffer;
/// individual meter values are atomic, but a whole meter frame is approximate.
#[derive(Clone, Default)]
pub struct Synth {
    controls: Arc<Mutex<Controls>>,
    published: Arc<Published>,
    meters: Arc<Meters>,
}
impl Synth {
    pub fn new() -> Self {
        Self::default()
    }
    fn update(&self, f: impl FnOnce(&mut Controls) -> Result<(), String>) -> Result<(), String> {
        let mut guard = self
            .controls
            .lock()
            .map_err(|_| "Synth control lock was poisoned".to_owned())?;
        let mut next = *guard;
        f(&mut next)?;
        next.params.validate().map_err(|e| e.to_string())?;
        *guard = next;
        self.published.store(next);
        Ok(())
    }
    pub fn voice_params(&self) -> Result<VoiceParams, String> {
        Ok(self
            .controls
            .lock()
            .map_err(|_| "Synth control lock was poisoned".to_owned())?
            .params)
    }
    pub fn params(&self, index: usize) -> Result<OscillatorParams, String> {
        self.voice_params()?
            .oscillators
            .get(index)
            .copied()
            .ok_or_else(|| "Invalid oscillator index".to_owned())
    }
    pub fn set_params(&self, index: usize, params: OscillatorParams) -> Result<(), String> {
        self.update(|c| {
            *c.params
                .oscillators
                .get_mut(index)
                .ok_or("Invalid oscillator index")? = params;
            Ok(())
        })
    }
    pub fn set_volume(&self, value: f32) -> Result<(), String> {
        self.update(|c| {
            c.params.volume = value;
            Ok(())
        })
    }
    pub fn set_global(&self, index: usize, value: f32) -> Result<(), String> {
        self.update(|c| {
            *c.params
                .globals
                .get_mut(index)
                .ok_or("Invalid global index")? = value;
            Ok(())
        })
    }
    pub fn set_lfo_wave(&self, wave: LfoWave) -> Result<(), String> {
        self.update(|c| {
            c.params.lfo_wave = wave;
            Ok(())
        })
    }
    pub fn set_lfo_retrigger(&self, retrigger: bool) -> Result<(), String> {
        self.update(|c| {
            c.params.lfo_retrigger = retrigger;
            Ok(())
        })
    }
    /// Both sources can address every target, including source controls.
    pub fn set_route(&self, target: usize, source: usize, depth: f32) -> Result<(), String> {
        self.update(|c| {
            *c.params
                .routes
                .get_mut(source)
                .and_then(|r| r.get_mut(target))
                .ok_or("Invalid modulation source or target")? = depth;
            Ok(())
        })
    }
    pub fn telemetry(&self) -> Telemetry {
        self.meters.load()
    }
    pub fn note_on(&self, note: u8) -> Result<(), String> {
        if note > 127 {
            return Err("MIDI note must be between 0 and 127".into());
        }
        self.update(|c| {
            c.note = Some(note);
            c.generation = c.generation.wrapping_add(1);
            Ok(())
        })
    }
    /// Starts the configured ADSR release. The audio stream remains running.
    pub fn note_off(&self) -> Result<(), String> {
        self.update(|c| {
            c.note = None;
            Ok(())
        })
    }
}

/// Owns the running stream: retain this value for as long as audio is needed.
/// Dropping it stops output. Device/format failures are returned by `start`;
/// later callback failures are available through `error` without locking.
pub struct AudioOutput {
    _stream: cpal::Stream,
    description: String,
    failed: Arc<AtomicBool>,
}

impl AudioOutput {
    pub fn start(synth: Synth) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "No default audio output device is available".to_owned())?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("Cannot read default audio configuration: {e}"))?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        if config.channels == 0 || config.sample_rate.0 == 0 {
            return Err("Audio device returned an invalid channel count or sample rate".to_owned());
        }
        let description = format!(
            "{} — {} Hz, {} channels, {format:?}",
            device
                .name()
                .map_err(|e| format!("Cannot read audio device name: {e}"))?,
            config.sample_rate.0,
            config.channels,
        );
        let failed = Arc::new(AtomicBool::new(false));
        let stream = match format {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config, synth, failed.clone()),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config, synth, failed.clone()),
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config, synth, failed.clone()),
            _ => {
                return Err(format!(
                    "Unsupported default audio sample format: {format:?}; expected f32, i16, or u16"
                ));
            }
        }?;
        stream
            .play()
            .map_err(|e| format!("Cannot start audio output: {e}"))?;
        Ok(Self {
            _stream: stream,
            description,
            failed,
        })
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    /// A sticky failure indicator. The backend error callback stores only an
    /// atomic flag; the UI/control thread allocates this message when requested.
    pub fn error(&self) -> Option<String> {
        self.failed.load(Ordering::Relaxed).then(|| {
            "Audio output stream failed. Check the output device and restart Plasma.".to_owned()
        })
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    synth: Synth,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64);
    let mut voice = Voice::new(f64::from(config.sample_rate.0), seed).map_err(|e| e.to_string())?;
    let mut previous = Controls::default();
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _: &cpal::OutputCallbackInfo| {
                if let Some(snapshot) = synth.published.load() {
                    let _ = voice.set_params(snapshot.params);
                    if let Some(note) = snapshot.note {
                        if snapshot.generation != previous.generation {
                            let _ = voice
                                .note_on(440.0 * 2.0_f64.powf((f64::from(note) - 69.0) / 12.0));
                        }
                    } else if previous.note.is_some() {
                        voice.note_off();
                    }
                    previous = snapshot;
                }
                let mut frames = output.chunks_exact_mut(channels);
                for frame in &mut frames {
                    let [left, right] = voice.next_frame();
                    if channels == 1 {
                        frame[0] = T::from_sample((left + right) * 0.5);
                    } else {
                        frame[0] = T::from_sample(left);
                        frame[1] = T::from_sample(right);
                        for sample in &mut frame[2..] {
                            *sample = T::from_sample(0.0);
                        }
                    }
                }
                for sample in frames.into_remainder() {
                    *sample = T::from_sample(0.0);
                }
                synth.meters.store(voice.telemetry());
            },
            move |_| failed.store(true, Ordering::Relaxed),
            None,
        )
        .map_err(|e| format!("Cannot create audio output stream: {e}"))
}
