use std::fmt;

/// Recoverable control, queue and device failures. Libraries return these;
/// binaries print or display them. The audio callback never constructs one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Kernel(plasma_kernel::Error),
    LockPoisoned,
    InvalidOscillator,
    InvalidGlobal,
    InvalidRoute,
    InvalidMidi,
    QueueFull,
    ConsumerBusy,
    NoOutputDevice,
    InvalidDeviceFormat,
    UnsupportedSampleFormat(String),
    DeviceConfig(String),
    DeviceName(String),
    StreamCreate(String),
    StreamStart(String),
    StreamFailed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kernel(error) => write!(f, "{error}"),
            Self::LockPoisoned => f.write_str("Synth control lock was poisoned"),
            Self::InvalidOscillator => f.write_str("Invalid oscillator index"),
            Self::InvalidGlobal => f.write_str("Invalid global index"),
            Self::InvalidRoute => f.write_str("Invalid modulation source or target"),
            Self::InvalidMidi => f.write_str("MIDI note and velocity must be between 0 and 127"),
            Self::QueueFull => f.write_str(
                "Note queue is full; all notes will be released at the next audio buffer",
            ),
            Self::ConsumerBusy => f.write_str("This Synth already has an audio consumer"),
            Self::NoOutputDevice => f.write_str("No default audio output device is available"),
            Self::InvalidDeviceFormat => {
                f.write_str("Audio device returned an invalid channel count or sample rate")
            }
            Self::UnsupportedSampleFormat(format) => write!(
                f,
                "Unsupported default audio sample format: {format}; expected f32, i16, or u16"
            ),
            Self::DeviceConfig(error) => {
                write!(f, "Cannot read default audio configuration: {error}")
            }
            Self::DeviceName(error) => write!(f, "Cannot read audio device name: {error}"),
            Self::StreamCreate(error) => write!(f, "Cannot create audio output stream: {error}"),
            Self::StreamStart(error) => write!(f, "Cannot start audio output: {error}"),
            Self::StreamFailed => f.write_str(
                "Audio output stream failed. Check the output device and restart Plasma.",
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Kernel(error) => Some(error),
            _ => None,
        }
    }
}

impl From<plasma_kernel::Error> for Error {
    fn from(error: plasma_kernel::Error) -> Self {
        Self::Kernel(error)
    }
}
