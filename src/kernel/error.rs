use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidSampleRate,
    InvalidFrequency,
    InvalidOscillatorIndex,
    InvalidParameter(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate => f.write_str("sample rate must be finite and positive"),
            Self::InvalidFrequency => f.write_str("frequency must be finite and nonnegative"),
            Self::InvalidOscillatorIndex => f.write_str("oscillator index must be in 0..3"),
            Self::InvalidParameter(name) => write!(f, "invalid oscillator parameter: {name}"),
        }
    }
}

impl std::error::Error for Error {}
