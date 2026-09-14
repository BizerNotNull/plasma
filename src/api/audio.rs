use crate::error::Error;
use crate::events::ConsumerLease;
use crate::synth::Synth;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use plasma_kernel::PolySynth;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

/// Offline or device-backed rendering through the same buffer-boundary path.
/// Construction exclusively claims this Synth's note FIFO until drop; starting
/// another renderer or AudioOutput returns an error. Rendering applies one
/// parameter snapshot and at most 128 queued events before producing samples.
/// Telemetry follows the most recently triggered active voice.
pub struct AudioRenderer {
    synth: Synth,
    lease: ConsumerLease,
    engine: PolySynth,
}

impl AudioRenderer {
    pub fn new(synth: Synth, sample_rate: f64, seed: u64) -> Result<Self, Error> {
        let lease = ConsumerLease::acquire(synth.notes.clone())?;
        Self::with_lease(synth, lease, sample_rate, seed)
    }

    pub(crate) fn with_lease(
        synth: Synth,
        lease: ConsumerLease,
        sample_rate: f64,
        seed: u64,
    ) -> Result<Self, Error> {
        let engine = PolySynth::new(sample_rate, seed)?;
        Ok(Self {
            synth,
            lease,
            engine,
        })
    }

    fn begin_buffer(&mut self) {
        if let Some(snapshot) = self.synth.published.load() {
            let _ = self.engine.set_params(snapshot.params);
        }
        self.lease.dispatch(&mut self.engine);
    }

    /// Renders stereo frames without allocating, locking, or waiting.
    pub fn render(&mut self, output: &mut [[f32; 2]]) {
        self.begin_buffer();
        self.engine.render(output);
        self.synth.meters.store(self.engine.telemetry());
    }

    pub fn active_voice_count(&self) -> usize {
        self.engine.active_voice_count()
    }

    pub(crate) fn render_interleaved<T>(&mut self, output: &mut [T], channels: usize)
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        self.begin_buffer();
        let mut frames = output.chunks_exact_mut(channels);
        for frame in &mut frames {
            let [left, right] = self.engine.next_frame();
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
        self.synth.meters.store(self.engine.telemetry());
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
    pub fn start(synth: Synth) -> Result<Self, Error> {
        let lease = ConsumerLease::acquire(synth.notes.clone())?;
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(Error::NoOutputDevice)?;
        let supported = device
            .default_output_config()
            .map_err(|e| Error::DeviceConfig(e.to_string()))?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        if config.channels == 0 || config.sample_rate.0 == 0 {
            return Err(Error::InvalidDeviceFormat);
        }
        let description = format!(
            "{} — {} Hz, {} channels, {format:?}",
            device
                .name()
                .map_err(|e| Error::DeviceName(e.to_string()))?,
            config.sample_rate.0,
            config.channels,
        );
        let failed = Arc::new(AtomicBool::new(false));
        let stream = match format {
            cpal::SampleFormat::F32 => {
                build_stream::<f32>(&device, &config, synth, lease, failed.clone())
            }
            cpal::SampleFormat::I16 => {
                build_stream::<i16>(&device, &config, synth, lease, failed.clone())
            }
            cpal::SampleFormat::U16 => {
                build_stream::<u16>(&device, &config, synth, lease, failed.clone())
            }
            _ => {
                return Err(Error::UnsupportedSampleFormat(format!("{format:?}")));
            }
        }?;
        stream
            .play()
            .map_err(|e| Error::StreamStart(e.to_string()))?;
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
    pub fn error(&self) -> Option<Error> {
        self.failed
            .load(Ordering::Relaxed)
            .then_some(Error::StreamFailed)
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    synth: Synth,
    lease: ConsumerLease,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64);
    let mut renderer =
        AudioRenderer::with_lease(synth, lease, f64::from(config.sample_rate.0), seed)?;
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _: &cpal::OutputCallbackInfo| {
                renderer.render_interleaved(output, channels);
            },
            move |_| failed.store(true, Ordering::Relaxed),
            None,
        )
        .map_err(|e| Error::StreamCreate(e.to_string()))
}
